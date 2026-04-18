//! hyperlight-unikraft: run Unikraft kernels on Hyperlight
//!
//! Provides a `Sandbox` wrapper around Hyperlight's `MultiUseSandbox` that
//! manages the kernel lifecycle: create → evolve (init) → snapshot → call.

pub mod ffi;
pub mod stderr_capture;

use anyhow::{anyhow, Result};
use hyperlight_host::func::Registerable;
use hyperlight_host::sandbox::uninitialized::GuestEnvironment;
use hyperlight_host::sandbox::SandboxConfiguration;
use hyperlight_host::sandbox::snapshot::Snapshot;
use hyperlight_host::{GuestBinary, MultiUseSandbox, UninitializedSandbox};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Magic header for cmdline embedded in initrd: "HLCMDLN\0"
const CMDLINE_MAGIC: &[u8; 8] = b"HLCMDLN\0";

const PAGE_SIZE: usize = 4096;

/// Guest VA for the initrd mapped via map_file_cow.
/// Computed dynamically in new_with_file_initrd to be after the
/// primary shared memory region, page-aligned.
/// Falls back to 2 GiB if the sandbox config doesn't have heap info.

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a Unikraft VM.
pub struct VmConfig {
    pub heap_size: u64,
    pub stack_size: u64,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            heap_size: 512 * 1024 * 1024,
            stack_size: 8 * 1024 * 1024,
        }
    }
}

impl VmConfig {
    pub fn with_heap_size(mut self, size: u64) -> Self {
        self.heap_size = size;
        self
    }

    pub fn with_stack_size(mut self, size: u64) -> Self {
        self.stack_size = size;
        self
    }

    fn sandbox_config(&self) -> SandboxConfiguration {
        let mut cfg = SandboxConfiguration::default();
        cfg.set_heap_size(self.heap_size);

        // Scratch holds page tables + CoW copies of writable pages touched at
        // runtime.  pt_estimate covers page tables; the base covers kernel
        // boot, CPIO extraction, ELF loading, and language runtime startup.
        // Use 25% of heap as base: large guests (e.g. Node.js) load 100+ MB
        // ELF binaries whose PT_LOAD segments trigger per-page CoW copies.
        let pt_estimate = ((self.heap_size as usize / (2 * 1024 * 1024)) + 16) * PAGE_SIZE;
        let base = std::cmp::max(self.heap_size as usize / 4, 64 * 1024 * 1024);
        let scratch = (pt_estimate + base).next_multiple_of(PAGE_SIZE);
        cfg.set_scratch_size(scratch);
        cfg
    }
}

/// Parse memory size string (e.g., "512Mi", "1Gi") into bytes.
pub fn parse_memory(mem_str: &str) -> Result<u64> {
    let s = mem_str.trim();
    if let Some(v) = s.strip_suffix("Gi") {
        Ok(v.parse::<u64>()? * 1024 * 1024 * 1024)
    } else if let Some(v) = s.strip_suffix("Mi") {
        Ok(v.parse::<u64>()? * 1024 * 1024)
    } else if let Some(v) = s.strip_suffix("Ki") {
        Ok(v.parse::<u64>()? * 1024)
    } else if let Some(v) = s.strip_suffix("G") {
        Ok(v.parse::<u64>()? * 1_000_000_000)
    } else if let Some(v) = s.strip_suffix("M") {
        Ok(v.parse::<u64>()? * 1_000_000)
    } else if let Some(v) = s.strip_suffix("K") {
        Ok(v.parse::<u64>()? * 1000)
    } else {
        s.parse().map_err(|e| anyhow!("Invalid memory format: {}", e))
    }
}

// ---------------------------------------------------------------------------
// Initrd cmdline prepend
// ---------------------------------------------------------------------------

/// Build init_data with cmdline + mapped initrd size (for map_file_cow mode).
/// The mapped file size is stored in the last 8 bytes of the page-aligned header.
fn build_cmdline_initdata(app_args: &[String], mapped_initrd_size: u64) -> Option<Vec<u8>> {
    let cmdline = app_args.join(" ");
    if cmdline.is_empty() && mapped_initrd_size == 0 {
        return None;
    }

    let cmdline_bytes = cmdline.as_bytes();
    let cmdline_len = cmdline_bytes.len() as u32;
    let header_size = 8 + 4 + cmdline_len as usize + 1;
    let padded = (header_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let mut buf = Vec::with_capacity(padded);
    buf.extend_from_slice(CMDLINE_MAGIC);
    buf.extend_from_slice(&cmdline_len.to_le_bytes());
    buf.extend_from_slice(cmdline_bytes);
    buf.push(0);
    // Pad to page boundary minus 8, then append mapped size
    buf.resize(padded - 8, 0);
    buf.extend_from_slice(&mapped_initrd_size.to_le_bytes());
    Some(buf)
}

/// Prepend application arguments as a cmdline header in the initrd.
pub fn prepend_cmdline_to_initrd(initrd: Option<&[u8]>, app_args: &[String]) -> Option<Vec<u8>> {
    let cmdline = app_args.join(" ");

    if cmdline.is_empty() && initrd.is_none() {
        return None;
    }
    if cmdline.is_empty() {
        return initrd.map(|d| d.to_vec());
    }

    let cmdline_bytes = cmdline.as_bytes();
    let cmdline_len = cmdline_bytes.len() as u32;
    let header_size = 8 + 4 + cmdline_len as usize + 1;
    let padded = (header_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let initrd_len = initrd.map(|d| d.len()).unwrap_or(0);
    let mut buf = Vec::with_capacity(padded + initrd_len);
    buf.extend_from_slice(CMDLINE_MAGIC);
    buf.extend_from_slice(&cmdline_len.to_le_bytes());
    buf.extend_from_slice(cmdline_bytes);
    buf.push(0);
    buf.resize(padded, 0);
    if let Some(data) = initrd {
        buf.extend_from_slice(data);
    }
    Some(buf)
}

// ---------------------------------------------------------------------------
// Tool dispatch (host functions callable from guest)
// ---------------------------------------------------------------------------

/// Registry of tool handlers callable from guest user-space via `/dev/hcall`.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value> + Send + Sync>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    pub fn register<F>(&mut self, name: &str, handler: F)
    where
        F: Fn(serde_json::Value) -> Result<serde_json::Value> + Send + Sync + 'static,
    {
        self.tools.insert(name.to_string(), Box::new(handler));
    }

    pub fn dispatch(&self, payload: &[u8]) -> Vec<u8> {
        let result = (|| -> Result<serde_json::Value> {
            let req: serde_json::Value = serde_json::from_slice(payload)?;
            let name = req["name"].as_str().ok_or_else(|| anyhow!("missing 'name'"))?;
            let args = req.get("args").cloned().unwrap_or(serde_json::Value::Null);
            let handler = self.tools.get(name).ok_or_else(|| anyhow!("unknown tool: {}", name))?;
            handler(args)
        })();
        let json = match result {
            Ok(v) => serde_json::json!({ "result": v }),
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        };
        serde_json::to_vec(&json).unwrap_or_else(|_| b"{\"error\":\"serialization failed\"}".to_vec())
    }
}

impl Default for ToolRegistry {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Filesystem sandbox — Phase A of host-mediated POSIX FS access
// ---------------------------------------------------------------------------

/// A sandboxed view of a host directory that the guest can read/write via
/// host function calls. All guest-supplied paths are resolved relative to
/// `root`; any attempt to escape the root (`..`, absolute paths, symlinks
/// pointing outside) is rejected.
///
/// Phase A deliberately exposes an explicit RPC surface: the guest calls
/// `fs_read` / `fs_write` / `fs_list` / `fs_stat` / `fs_mkdir` / `fs_unlink`
/// by name. Phase B will add a transparent POSIX shim in Unikraft that
/// forwards VFS operations to these same host handlers.
#[derive(Clone)]
pub struct FsSandbox {
    root: std::path::PathBuf,
}

impl FsSandbox {
    /// Create a new sandbox rooted at `root` (must be an existing directory).
    pub fn new<P: AsRef<Path>>(root: P) -> Result<Self> {
        let root = std::fs::canonicalize(root.as_ref())
            .map_err(|e| anyhow!("canonicalize mount root {:?}: {}", root.as_ref(), e))?;
        if !root.is_dir() {
            return Err(anyhow!("mount root is not a directory: {:?}", root));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path { &self.root }

    /// Resolve a guest-supplied path to a host path that is guaranteed to
    /// live under `root`. Returns an error on any escape attempt.
    ///
    /// Strategy:
    ///  - Strip any leading `/` so guest paths are relative to the mount.
    ///  - Logically normalise `.` / `..` without touching the filesystem.
    ///  - If the resolved path exists, `canonicalize` to follow symlinks
    ///    and verify the target is under `root`.
    ///  - If it doesn't exist (e.g. creating a new file), canonicalise the
    ///    nearest existing ancestor and append the remaining components —
    ///    this still catches symlinked ancestors that escape the root.
    fn resolve(&self, guest_path: &str) -> Result<std::path::PathBuf> {
        use std::path::{Component, PathBuf};
        let rel = guest_path.trim_start_matches('/');
        let joined = self.root.join(rel);
        // Logical resolution first: reject ".." once we're rooted.
        let mut logical = PathBuf::new();
        for c in joined.components() {
            match c {
                Component::ParentDir => {
                    if !logical.pop() {
                        return Err(anyhow!("path escapes mount root: {:?}", guest_path));
                    }
                }
                Component::CurDir => {}
                c => logical.push(c),
            }
        }
        if !logical.starts_with(&self.root) {
            return Err(anyhow!("path escapes mount root: {:?}", guest_path));
        }
        // Symlink check: canonicalise the deepest existing ancestor.
        let mut existing = logical.as_path();
        let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
        let resolved_ancestor = loop {
            if existing.exists() {
                break std::fs::canonicalize(existing)
                    .map_err(|e| anyhow!("canonicalize {:?}: {}", existing, e))?;
            }
            let Some(name) = existing.file_name() else {
                return Err(anyhow!("path has no existing ancestor: {:?}", logical));
            };
            tail.push(name);
            existing = existing.parent()
                .ok_or_else(|| anyhow!("path has no existing ancestor: {:?}", logical))?;
        };
        if !resolved_ancestor.starts_with(&self.root) {
            return Err(anyhow!("path escapes mount root (symlink): {:?}", guest_path));
        }
        let mut out = resolved_ancestor;
        for name in tail.into_iter().rev() {
            out.push(name);
        }
        Ok(out)
    }

    /// Register all FS tool handlers (`fs_read`, `fs_write`, …) on `registry`.
    pub fn register(self, registry: &mut ToolRegistry) {
        use serde_json::json;

        let s = self.clone();
        registry.register("fs_read", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_read: missing 'path'"))?;
            let target = s.resolve(path)?;
            let text = std::fs::read_to_string(&target)
                .map_err(|e| anyhow!("fs_read {:?}: {}", path, e))?;
            Ok(json!({ "text": text }))
        });

        let s = self.clone();
        registry.register("fs_write", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_write: missing 'path'"))?;
            let text = args["text"].as_str()
                .ok_or_else(|| anyhow!("fs_write: missing 'text'"))?;
            let append = args["append"].as_bool().unwrap_or(false);
            let target = s.resolve(path)?;
            // Create parent dirs? No — guest must fs_mkdir explicitly.
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(!append)
                .append(append)
                .open(&target)
                .map_err(|e| anyhow!("fs_write {:?}: {}", path, e))?;
            f.write_all(text.as_bytes())
                .map_err(|e| anyhow!("fs_write {:?}: {}", path, e))?;
            Ok(json!({ "bytes_written": text.len() }))
        });

        let s = self.clone();
        registry.register("fs_list", move |args| {
            let path = args["path"].as_str().unwrap_or("");
            let target = s.resolve(path)?;
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(&target)
                .map_err(|e| anyhow!("fs_list {:?}: {}", path, e))?
            {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let ft = entry.file_type()?;
                entries.push(json!({
                    "name": name,
                    "is_dir": ft.is_dir(),
                    "is_file": ft.is_file(),
                    "is_symlink": ft.is_symlink(),
                }));
            }
            Ok(json!({ "entries": entries }))
        });

        let s = self.clone();
        registry.register("fs_stat", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_stat: missing 'path'"))?;
            let target = s.resolve(path)?;
            let md = std::fs::metadata(&target)
                .map_err(|e| anyhow!("fs_stat {:?}: {}", path, e))?;
            Ok(json!({
                "size": md.len(),
                "is_dir": md.is_dir(),
                "is_file": md.is_file(),
            }))
        });

        let s = self.clone();
        registry.register("fs_mkdir", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_mkdir: missing 'path'"))?;
            let parents = args["parents"].as_bool().unwrap_or(false);
            let target = s.resolve(path)?;
            if parents {
                std::fs::create_dir_all(&target)
            } else {
                std::fs::create_dir(&target)
            }
            .map_err(|e| anyhow!("fs_mkdir {:?}: {}", path, e))?;
            Ok(json!({}))
        });

        // fs_read_bytes / fs_write_bytes — binary variants for the Phase B
        // transparent POSIX shim. Bytes are base64-encoded in the JSON
        // payload so arbitrary binary content round-trips intact.
        //
        // fs_read_bytes args: { path, offset?, len? } → { data: "<base64>", eof: bool }
        // fs_write_bytes args: { path, data: "<base64>", offset?, append? } → { bytes_written }
        let s = self.clone();
        registry.register("fs_read_bytes", move |args| {
            use base64::Engine;
            use std::io::{Read, Seek, SeekFrom};
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_read_bytes: missing 'path'"))?;
            let offset = args["offset"].as_u64().unwrap_or(0);
            let want = args["len"].as_u64().unwrap_or(65536);
            let target = s.resolve(path)?;
            let mut f = std::fs::File::open(&target)
                .map_err(|e| anyhow!("fs_read_bytes {:?}: {}", path, e))?;
            if offset > 0 {
                f.seek(SeekFrom::Start(offset))
                    .map_err(|e| anyhow!("fs_read_bytes seek {:?}: {}", path, e))?;
            }
            let mut buf = vec![0u8; want as usize];
            let n = f.read(&mut buf)
                .map_err(|e| anyhow!("fs_read_bytes {:?}: {}", path, e))?;
            buf.truncate(n);
            let eof = n < want as usize;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&buf);
            Ok(json!({ "data": encoded, "eof": eof, "bytes_read": n }))
        });

        let s = self.clone();
        registry.register("fs_write_bytes", move |args| {
            use base64::Engine;
            use std::io::{Seek, SeekFrom, Write};
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_write_bytes: missing 'path'"))?;
            let data_b64 = args["data"].as_str()
                .ok_or_else(|| anyhow!("fs_write_bytes: missing 'data'"))?;
            let data = base64::engine::general_purpose::STANDARD.decode(data_b64)
                .map_err(|e| anyhow!("fs_write_bytes: bad base64: {}", e))?;
            let offset = args["offset"].as_u64();
            let append = args["append"].as_bool().unwrap_or(false);
            let target = s.resolve(path)?;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(offset.is_none() && !append)
                .append(append)
                .open(&target)
                .map_err(|e| anyhow!("fs_write_bytes {:?}: {}", path, e))?;
            if let Some(off) = offset {
                if !append {
                    f.seek(SeekFrom::Start(off))
                        .map_err(|e| anyhow!("fs_write_bytes seek {:?}: {}", path, e))?;
                }
            }
            f.write_all(&data)
                .map_err(|e| anyhow!("fs_write_bytes {:?}: {}", path, e))?;
            Ok(json!({ "bytes_written": data.len() }))
        });

        let s = self.clone();
        registry.register("fs_truncate", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_truncate: missing 'path'"))?;
            let length = args["length"].as_u64()
                .ok_or_else(|| anyhow!("fs_truncate: missing 'length'"))?;
            let target = s.resolve(path)?;
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open(&target)
                .map_err(|e| anyhow!("fs_truncate {:?}: {}", path, e))?;
            f.set_len(length)
                .map_err(|e| anyhow!("fs_truncate {:?}: {}", path, e))?;
            Ok(json!({}))
        });

        let s = self.clone();
        registry.register("fs_unlink", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_unlink: missing 'path'"))?;
            let target = s.resolve(path)?;
            let md = std::fs::metadata(&target)
                .map_err(|e| anyhow!("fs_unlink {:?}: {}", path, e))?;
            if md.is_dir() {
                std::fs::remove_dir(&target)
            } else {
                std::fs::remove_file(&target)
            }
            .map_err(|e| anyhow!("fs_unlink {:?}: {}", path, e))?;
            Ok(json!({}))
        });
    }
}

/// Internal helper: assemble the final tool registry from caller-supplied
/// tools plus any preopened directories. Returns `None` if neither produces
/// any tools (so the `__dispatch` host function isn't registered in vain).
fn build_tools(
    user_tools: Option<ToolRegistry>,
    preopened_dir: Option<&Path>,
) -> Result<Option<ToolRegistry>> {
    match (user_tools, preopened_dir) {
        (None, None) => Ok(None),
        (Some(t), None) => Ok(Some(t)),
        (user, Some(dir)) => {
            let mut registry = user.unwrap_or_default();
            let fs = FsSandbox::new(dir)?;
            fs.register(&mut registry);
            Ok(Some(registry))
        }
    }
}

// ---------------------------------------------------------------------------
// Sandbox — the primary API
// ---------------------------------------------------------------------------

/// A Unikraft sandbox backed by Hyperlight's `MultiUseSandbox`.
///
/// Lifecycle:
///   1. `Sandbox::new()` — creates the VM and runs guest init (`evolve`)
///   2. Automatically takes a snapshot after init
///   3. `run()` — (future) calls a guest function; currently a no-op since
///      unikernels do all work during init
///   4. `restore()` — restores to the post-init snapshot for the next run
pub struct Sandbox {
    inner: MultiUseSandbox,
    /// Post-init snapshot for fast restore between calls.
    snapshot: Option<Arc<Snapshot>>,
    /// File mapping to re-register after snapshot restore.
    /// Snapshot restore unmaps all non-snapshot regions.
    file_mapping_path: Option<std::path::PathBuf>,
    file_mapping_base: u64,
}

impl Sandbox {
    /// Create a new sandbox, evolving the guest through its init phase.
    ///
    /// For unikernels that run their entire program during init, this
    /// completes the full execution.  For guests that export callable
    /// functions, init sets up the runtime and the host can then call
    /// guest functions via `call()`.
    pub fn new(
        kernel_path: &Path,
        initrd: Option<&[u8]>,
        app_args: &[String],
        config: VmConfig,
        tools: Option<ToolRegistry>,
        preopened_dir: Option<&Path>,
    ) -> Result<Self> {
        if !kernel_path.exists() {
            return Err(anyhow!("Kernel not found: {:?}", kernel_path));
        }

        let extended_initrd = prepend_cmdline_to_initrd(initrd, app_args);
        let env = GuestEnvironment::new(
            GuestBinary::FilePath(kernel_path.to_string_lossy().to_string()),
            extended_initrd.as_deref(),
        );

        let mut usbox = UninitializedSandbox::new(env, Some(config.sandbox_config()))?;

        let tools = build_tools(tools, preopened_dir)?;

        // Register tool dispatch host function if tools are provided
        if let Some(tools) = tools {
            let tools = Arc::new(tools);
            let tools_ref = tools.clone();
            usbox.register_host_function(
                "__dispatch",
                move |payload: Vec<u8>| -> Vec<u8> { tools_ref.dispatch(&payload) },
            )?;
        }

        Self::finish_evolve(usbox, None, 0)
    }

    /// Create a new sandbox with initrd mapped via zero-copy CoW file mapping.
    ///
    /// Instead of copying the initrd into snapshot memory, maps the file
    /// directly into guest address space at INITRD_MAP_BASE. The guest's
    /// demand-paging handler creates page table entries on first access.
    pub fn new_with_file_initrd(
        kernel_path: &Path,
        initrd_path: Option<&Path>,
        app_args: &[String],
        config: VmConfig,
        tools: Option<ToolRegistry>,
        preopened_dir: Option<&Path>,
    ) -> Result<Self> {
        if !kernel_path.exists() {
            return Err(anyhow!("Kernel not found: {:?}", kernel_path));
        }

        // Get file size before creating sandbox
        let mapped_size = match initrd_path {
            Some(path) if path.exists() => std::fs::metadata(path)?.len(),
            Some(path) => return Err(anyhow!("Initrd not found: {:?}", path)),
            None => 0,
        };

        // Build init_data with cmdline + mapped file size
        let cmdline_data = build_cmdline_initdata(app_args, mapped_size);
        let env = GuestEnvironment::new(
            GuestBinary::FilePath(kernel_path.to_string_lossy().to_string()),
            cmdline_data.as_deref(),
        );

        let mut usbox = UninitializedSandbox::new(env, Some(config.sandbox_config()))?;

        // Map the initrd file (zero-copy via mmap)
        // Place at 3 GiB — high enough to not overlap any reasonable
        // primary shared memory region, within the 4 GiB identity map.
        const INITRD_MAP_BASE: u64 = 0xC000_0000; // 3 GiB
        if let Some(path) = initrd_path {
            usbox.map_file_cow(path, INITRD_MAP_BASE, Some("initrd"))?;
        }

        let tools = build_tools(tools, preopened_dir)?;

        // Register tool dispatch if needed
        if let Some(tools) = tools {
            let tools = Arc::new(tools);
            let tools_ref = tools.clone();
            usbox.register_host_function(
                "__dispatch",
                move |payload: Vec<u8>| -> Vec<u8> { tools_ref.dispatch(&payload) },
            )?;
        }

        Self::finish_evolve(
            usbox,
            initrd_path.map(|p| p.to_path_buf()),
            INITRD_MAP_BASE,
        )
    }

    fn finish_evolve(
        usbox: UninitializedSandbox,
        file_mapping_path: Option<std::path::PathBuf>,
        file_mapping_base: u64,
    ) -> Result<Self> {
        let mut inner = usbox.evolve()?;
        let snapshot = inner.snapshot().ok();
        Ok(Self {
            inner,
            snapshot,
            file_mapping_path,
            file_mapping_base,
        })
    }

    /// Restore the sandbox to its post-init snapshot.
    ///
    /// This is a fast operation (host-level CoW via mmap) that resets all
    /// guest memory to the state captured after init.
    pub fn restore(&mut self) -> Result<()> {
        if let Some(ref snap) = self.snapshot {
            self.inner.restore(snap.clone())?;
        }
        // Re-register file mapping after restore (snapshot restore
        // unmaps all non-snapshot regions including file mappings)
        if let Some(ref path) = self.file_mapping_path {
            self.inner.map_file_cow(path, self.file_mapping_base, Some("initrd"))?;
        }
        Ok(())
    }

    /// Call the dispatch function to re-run the application.
    ///
    /// Requires a prior `restore()` to reset guest state.
    /// The dispatch function pops the FunctionCall from input,
    /// runs the application, pushes a void result, and halts.
    pub fn call_run(&mut self) -> Result<()> {
        // call() with Void return type — the function name doesn't matter
        // to the guest (it ignores it and just runs the app).
        let _: () = self.inner.call("run", ())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Convenience: run_vm (single-shot execution)
// ---------------------------------------------------------------------------

/// Run a Unikraft kernel to completion (single-shot).
///
/// This is the simple path for unikernels that do all work during init.
/// The entire program executes during `Sandbox::new()`.
pub fn run_vm(
    kernel_path: &Path,
    initrd: Option<&[u8]>,
    app_args: &[String],
    config: VmConfig,
) -> Result<()> {
    let _ = Sandbox::new(kernel_path, initrd, app_args, config, None, None)?;
    Ok(())
}

/// Run a Unikraft kernel with tool dispatch support.
pub fn run_vm_with_tools(
    kernel_path: &Path,
    initrd: Option<&[u8]>,
    app_args: &[String],
    config: VmConfig,
    tools: ToolRegistry,
) -> Result<()> {
    let _ = Sandbox::new(kernel_path, initrd, app_args, config, Some(tools), None)?;
    Ok(())
}

/// Run a Unikraft kernel with a preopened host directory.
///
/// The guest's `lib/hostfs` mounts `host_dir` at `/host`; unmodified POSIX
/// calls route through the `FsSandbox` tool handlers. Escape attempts (via
/// `..` or symlinks that point outside `host_dir`) are rejected host-side.
pub fn run_vm_with_preopen(
    kernel_path: &Path,
    initrd: Option<&[u8]>,
    app_args: &[String],
    config: VmConfig,
    host_dir: &Path,
) -> Result<()> {
    let _ = Sandbox::new(kernel_path, initrd, app_args, config, None, Some(host_dir))?;
    Ok(())
}

/// Output captured from a VM execution.
pub struct VmOutput {
    pub output: String,
    pub setup_time: Duration,
    pub evolve_time: Duration,
}

/// Run a Unikraft kernel and capture its console output.
///
/// Unikraft console output goes through Hyperlight's port I/O to host stderr.
/// This function redirects stderr to a temp file during the call phase to
/// capture it.  The Unikraft dispatch lifecycle is:
///   evolve (boot+init+snapshot) → restore → call_run (app output here)
pub fn run_vm_capture_output(
    kernel_path: &Path,
    initrd: Option<&[u8]>,
    app_args: &[String],
    config: VmConfig,
) -> Result<VmOutput> {
    let setup_start = std::time::Instant::now();

    // Phase 1: evolve — boots the kernel and takes a post-init snapshot.
    // No application output happens here.
    let mut sandbox = Sandbox::new(kernel_path, initrd, app_args, config, None, None)?;
    let setup_time = setup_start.elapsed();

    // Redirect stderr to a temp file before the call phase
    let capture_file = std::env::temp_dir().join(format!("hl-capture-{}", std::process::id()));
    let capture = stderr_capture::Capture::redirect_to_file(&capture_file)?;

    // Phase 2: restore + call — application runs and produces output
    let evolve_start = std::time::Instant::now();
    sandbox.restore()?;
    let call_result = sandbox.call_run();
    let evolve_time = evolve_start.elapsed();

    // Restore stderr
    capture.restore()?;

    // Read captured output
    let captured = std::fs::read(&capture_file).unwrap_or_default();
    let _ = std::fs::remove_file(&capture_file);
    let captured = String::from_utf8_lossy(&captured).into_owned();

    if let Err(e) = call_result {
        return Err(anyhow!(
            "VM call failed: {}\n--- captured output ---\n{}",
            e,
            captured
        ));
    }

    Ok(VmOutput {
        output: captured,
        setup_time,
        evolve_time,
    })
}

// ---------------------------------------------------------------------------
// FsSandbox tests — prove that host-side path resolution rejects escapes.
//
// These cover both attack vectors the host can see: lexical ".." /
// absolute paths passed in an RPC arg, and symlinks inside the mount
// that point outside it.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "hl-fs-sandbox-{}-{}",
            label,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn resolve_rejects_parent_escape() {
        let root = tmpdir("parent");
        let fs = FsSandbox::new(&root).unwrap();
        let err = fs.resolve("../etc/passwd").unwrap_err().to_string();
        assert!(err.contains("escapes mount root"), "{err}");
    }

    #[test]
    fn resolve_rejects_deep_parent_escape() {
        let root = tmpdir("deep");
        let fs = FsSandbox::new(&root).unwrap();
        let err = fs.resolve("a/b/../../../outside").unwrap_err().to_string();
        assert!(err.contains("escapes mount root"), "{err}");
    }

    #[test]
    fn resolve_treats_absolute_paths_as_mount_relative() {
        // A leading '/' is stripped, so "/etc/passwd" becomes
        // "etc/passwd" under the mount — not the host's /etc/passwd.
        let root = tmpdir("abs");
        fs::create_dir(root.join("etc")).unwrap();
        fs::write(root.join("etc/passwd"), "fake").unwrap();
        let fs_sb = FsSandbox::new(&root).unwrap();
        let resolved = fs_sb.resolve("/etc/passwd").unwrap();
        assert_eq!(resolved, root.join("etc/passwd"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let root = tmpdir("symlink");
        let outside = tmpdir("outside");
        fs::write(outside.join("secret"), "nope").unwrap();
        symlink(outside.join("secret"), root.join("leak")).unwrap();
        let fs_sb = FsSandbox::new(&root).unwrap();
        let err = fs_sb.resolve("leak").unwrap_err().to_string();
        assert!(err.contains("escapes mount root"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_symlink_escape_via_ancestor() {
        // A symlinked parent directory is just as effective: any child
        // under it resolves outside the root.
        use std::os::unix::fs::symlink;
        let root = tmpdir("ancestor");
        let outside = tmpdir("outside-anc");
        symlink(&outside, root.join("shortcut")).unwrap();
        let fs_sb = FsSandbox::new(&root).unwrap();
        let err = fs_sb.resolve("shortcut/anything").unwrap_err().to_string();
        assert!(err.contains("escapes mount root"), "{err}");
    }

    #[test]
    fn resolve_allows_paths_under_the_root() {
        let root = tmpdir("allow");
        let fs = FsSandbox::new(&root).unwrap();
        let resolved = fs.resolve("subdir/file.txt").unwrap();
        assert!(resolved.starts_with(&root), "{resolved:?}");
    }

    #[test]
    fn fs_read_over_dispatch_rejects_escape() {
        // End-to-end through the tool registry: the error surface the
        // guest actually sees.
        let root = tmpdir("dispatch");
        let mut reg = ToolRegistry::new();
        FsSandbox::new(&root).unwrap().register(&mut reg);

        let req = br#"{"name":"fs_read","args":{"path":"../outside.txt"}}"#;
        let resp = reg.dispatch(req);
        let s = std::str::from_utf8(&resp).unwrap();
        assert!(s.contains("\"error\""), "{s}");
        assert!(s.contains("escapes mount root"), "{s}");
    }

    #[test]
    fn fs_write_then_read_roundtrip() {
        let root = tmpdir("roundtrip");
        let mut reg = ToolRegistry::new();
        FsSandbox::new(&root).unwrap().register(&mut reg);

        let w = br#"{"name":"fs_write","args":{"path":"hello.txt","text":"hi"}}"#;
        let resp = reg.dispatch(w);
        let s = std::str::from_utf8(&resp).unwrap();
        assert!(s.contains("\"bytes_written\":2"), "{s}");

        let r = br#"{"name":"fs_read","args":{"path":"hello.txt"}}"#;
        let resp = reg.dispatch(r);
        let s = std::str::from_utf8(&resp).unwrap();
        assert!(s.contains("\"text\":\"hi\""), "{s}");
    }
}
