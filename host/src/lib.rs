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

/// Magic header for the optional hostfs mount point TLV that follows the
/// cmdline (same init_data page).
const MOUNT_MAGIC: &[u8; 8] = b"HLHSMNT\0";

/// Magic header for the optional wall-clock-at-boot TLV. Value is a
/// little-endian u64 of nanoseconds since the Unix epoch. The guest
/// adds its own monotonic delta at read time, so `time.time()` returns
/// a sensible wall time without any host round-trip per call.
const WALLTIME_MAGIC: &[u8; 8] = b"HLWALL0\0";

const PAGE_SIZE: usize = 4096;

/// Guest paths that would shadow the kernel's own ramfs and break the VM.
/// Reject these early on the host before we even boot the guest.
const RESERVED_GUEST_MOUNTPOINTS: &[&str] = &[
    "/", "/bin", "/dev", "/proc", "/sys", "/usr",
];

/// A preopened host directory exposed to the guest.
///
/// Semantics mirror Wasmtime's `preopened_dir`: `host_dir` is canonicalised
/// at construction time and used as the sandbox root for every RPC the
/// guest issues; `guest_path` is the absolute path inside the guest where
/// `lib/hostfs` mounts it.
#[derive(Clone, Debug)]
pub struct Preopen {
    pub host_dir: std::path::PathBuf,
    pub guest_path: String,
}

impl Preopen {
    /// Construct a preopen. `guest_path` must be absolute (`/something`)
    /// and not shadow a reserved kernel directory — see
    /// `RESERVED_GUEST_MOUNTPOINTS`.
    pub fn new<P: AsRef<Path>>(host_dir: P, guest_path: impl Into<String>) -> Result<Self> {
        let guest_path = guest_path.into();
        if !guest_path.starts_with('/') {
            return Err(anyhow!(
                "guest mount path {:?} must be absolute",
                guest_path
            ));
        }
        for reserved in RESERVED_GUEST_MOUNTPOINTS {
            if guest_path == *reserved
                || guest_path.starts_with(&format!("{}/", reserved))
            {
                return Err(anyhow!(
                    "refusing to mount at guest path {:?}: shadows reserved kernel dir",
                    guest_path
                ));
            }
        }
        let host_dir = std::fs::canonicalize(host_dir.as_ref()).map_err(|e| {
            anyhow!("canonicalize preopen host dir {:?}: {}", host_dir.as_ref(), e)
        })?;
        Ok(Self { host_dir, guest_path })
    }

    /// Parse a `HOST[:GUEST]` CLI argument. When `GUEST` is omitted the
    /// default guest mount point is `/host`.
    pub fn parse_cli(s: &str) -> Result<Self> {
        // Windows absolute paths contain ':'. Disambiguate by splitting on
        // the *last* colon only if the right side looks like an absolute
        // guest path (starts with /). Otherwise treat the whole string as
        // the host dir.
        if let Some(idx) = s.rfind(':') {
            let (host, guest) = s.split_at(idx);
            let guest = &guest[1..];
            if guest.starts_with('/') {
                return Self::new(host, guest);
            }
        }
        Self::new(s, "/host")
    }
}

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

/// Serialize the shared "cmdline + preopens + wall clock" TLV block into `buf`.
///
/// Layout:
///   [HLCMDLN\0][cmdline_len u32][cmdline…][\0]
///   [HLHSMNT\0][count u32]([path_len u32][path…][\0])*count  (optional block)
///   [HLWALL0\0][8 u32][wall_ns_le u64]
///
/// Callers are responsible for any trailing padding / metadata (e.g. the
/// mapped-initrd-size footer used by `build_cmdline_initdata`).
fn write_cmdline_mount_tlv(
    buf: &mut Vec<u8>,
    cmdline_bytes: &[u8],
    preopens: &[Preopen],
) {
    let cmdline_len = cmdline_bytes.len() as u32;
    buf.extend_from_slice(CMDLINE_MAGIC);
    buf.extend_from_slice(&cmdline_len.to_le_bytes());
    buf.extend_from_slice(cmdline_bytes);
    buf.push(0);

    if !preopens.is_empty() {
        buf.extend_from_slice(MOUNT_MAGIC);
        buf.extend_from_slice(&(preopens.len() as u32).to_le_bytes());
        for p in preopens {
            let b = p.guest_path.as_bytes();
            buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
            buf.extend_from_slice(b);
            buf.push(0);
        }
    }

    // Wall clock: read the host's time once at VM build time and embed
    // as ns since epoch. The guest will add its own monotonic delta.
    let wall_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    buf.extend_from_slice(WALLTIME_MAGIC);
    buf.extend_from_slice(&8u32.to_le_bytes());
    buf.extend_from_slice(&wall_ns.to_le_bytes());
}

/// Build init_data with cmdline + preopens + mapped initrd size (for
/// map_file_cow mode). The mapped file size is stored in the last 8
/// bytes of the page-aligned header.
fn build_cmdline_initdata(
    app_args: &[String],
    mapped_initrd_size: u64,
    preopens: &[Preopen],
) -> Option<Vec<u8>> {
    let cmdline = app_args.join(" ");
    if cmdline.is_empty() && mapped_initrd_size == 0 && preopens.is_empty() {
        return None;
    }

    let cmdline_bytes = cmdline.as_bytes();
    let mut buf = Vec::new();
    write_cmdline_mount_tlv(&mut buf, cmdline_bytes, preopens);

    let padded = (buf.len() + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    buf.resize(padded - 8, 0);
    buf.extend_from_slice(&mapped_initrd_size.to_le_bytes());
    Some(buf)
}

/// Prepend application arguments + preopens as a header in the initrd.
pub fn prepend_cmdline_to_initrd(
    initrd: Option<&[u8]>,
    app_args: &[String],
    preopens: &[Preopen],
) -> Option<Vec<u8>> {
    let cmdline = app_args.join(" ");

    if cmdline.is_empty() && initrd.is_none() && preopens.is_empty() {
        return None;
    }
    if cmdline.is_empty() && preopens.is_empty() {
        return initrd.map(|d| d.to_vec());
    }

    let cmdline_bytes = cmdline.as_bytes();
    let mut buf = Vec::new();
    write_cmdline_mount_tlv(&mut buf, cmdline_bytes, preopens);

    let padded = (buf.len() + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
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
    pub(crate) fn resolve(&self, guest_path: &str) -> Result<std::path::PathBuf> {
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
/// tools plus any preopened directories. Multiple preopens share one set
/// of fs_* tool handlers that route by guest-path prefix: the handler
/// inspects the `path` argument, finds the matching preopen, and
/// resolves the tail under that host directory.
fn build_tools(
    user_tools: Option<ToolRegistry>,
    preopens: &[Preopen],
) -> Result<Option<ToolRegistry>> {
    if preopens.is_empty() {
        return Ok(user_tools);
    }
    let mut registry = user_tools.unwrap_or_default();
    let router = FsRouter::new(preopens)?;
    router.register(&mut registry);
    Ok(Some(registry))
}

/// Routes incoming fs_* tool calls to the matching `FsSandbox` by
/// matching the guest-supplied path against each preopen's guest path.
#[derive(Clone)]
struct FsRouter {
    entries: Vec<(String, FsSandbox)>,
}

impl FsRouter {
    fn new(preopens: &[Preopen]) -> Result<Self> {
        let mut entries = Vec::with_capacity(preopens.len());
        for p in preopens {
            entries.push((p.guest_path.clone(), FsSandbox::new(&p.host_dir)?));
        }
        // Sort by descending prefix length so longer matches win (e.g.
        // /data/public should match before /data).
        entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        Ok(Self { entries })
    }

    /// Pick the preopen matching `path` and return its sandbox plus
    /// the path-relative-to-that-sandbox.
    fn route<'a>(&'a self, path: &'a str) -> Result<(&'a FsSandbox, &'a str)> {
        for (prefix, fs) in &self.entries {
            if path == prefix {
                return Ok((fs, ""));
            }
            if let Some(tail) = path
                .strip_prefix(prefix)
                .and_then(|t| t.strip_prefix('/'))
            {
                return Ok((fs, tail));
            }
        }
        Err(anyhow!(
            "path {:?} does not match any preopened mount",
            path
        ))
    }

    fn register(self, registry: &mut ToolRegistry) {
        use serde_json::json;

        let r = self.clone();
        registry.register("fs_read", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_read: missing 'path'"))?;
            let (fs, rel) = r.route(path)?;
            let target = fs.resolve(rel)?;
            let text = std::fs::read_to_string(&target)
                .map_err(|e| anyhow!("fs_read {:?}: {}", path, e))?;
            Ok(json!({ "text": text }))
        });

        let r = self.clone();
        registry.register("fs_write", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_write: missing 'path'"))?;
            let text = args["text"].as_str()
                .ok_or_else(|| anyhow!("fs_write: missing 'text'"))?;
            let append = args["append"].as_bool().unwrap_or(false);
            let (fs, rel) = r.route(path)?;
            let target = fs.resolve(rel)?;
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true).create(true)
                .truncate(!append).append(append)
                .open(&target)
                .map_err(|e| anyhow!("fs_write {:?}: {}", path, e))?;
            f.write_all(text.as_bytes())
                .map_err(|e| anyhow!("fs_write {:?}: {}", path, e))?;
            Ok(json!({ "bytes_written": text.len() }))
        });

        let r = self.clone();
        registry.register("fs_list", move |args| {
            let path = args["path"].as_str().unwrap_or("");
            let (fs, rel) = r.route(path)?;
            let target = fs.resolve(rel)?;
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(&target)
                .map_err(|e| anyhow!("fs_list {:?}: {}", path, e))?
            {
                let entry = entry?;
                let ft = entry.file_type()?;
                entries.push(json!({
                    "name": entry.file_name().to_string_lossy().into_owned(),
                    "is_dir": ft.is_dir(),
                    "is_file": ft.is_file(),
                    "is_symlink": ft.is_symlink(),
                }));
            }
            Ok(json!({ "entries": entries }))
        });

        let r = self.clone();
        registry.register("fs_stat", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_stat: missing 'path'"))?;
            let (fs, rel) = r.route(path)?;
            let target = fs.resolve(rel)?;
            let md = std::fs::metadata(&target)
                .map_err(|e| anyhow!("fs_stat {:?}: {}", path, e))?;
            Ok(json!({
                "size": md.len(),
                "is_dir": md.is_dir(),
                "is_file": md.is_file(),
            }))
        });

        let r = self.clone();
        registry.register("fs_read_bytes", move |args| {
            use base64::Engine;
            use std::io::{Read, Seek, SeekFrom};
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_read_bytes: missing 'path'"))?;
            let offset = args["offset"].as_u64().unwrap_or(0);
            let want = args["len"].as_u64().unwrap_or(65536);
            let (fs, rel) = r.route(path)?;
            let target = fs.resolve(rel)?;
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
            Ok(json!({
                "data": base64::engine::general_purpose::STANDARD.encode(&buf),
                "eof": eof, "bytes_read": n,
            }))
        });

        let r = self.clone();
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
            let (fs, rel) = r.route(path)?;
            let target = fs.resolve(rel)?;
            let mut f = std::fs::OpenOptions::new()
                .write(true).create(true)
                .truncate(offset.is_none() && !append).append(append)
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

        let r = self.clone();
        registry.register("fs_truncate", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_truncate: missing 'path'"))?;
            let length = args["length"].as_u64()
                .ok_or_else(|| anyhow!("fs_truncate: missing 'length'"))?;
            let (fs, rel) = r.route(path)?;
            let target = fs.resolve(rel)?;
            let f = std::fs::OpenOptions::new().write(true).open(&target)
                .map_err(|e| anyhow!("fs_truncate {:?}: {}", path, e))?;
            f.set_len(length)
                .map_err(|e| anyhow!("fs_truncate {:?}: {}", path, e))?;
            Ok(json!({}))
        });

        let r = self.clone();
        registry.register("fs_mkdir", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_mkdir: missing 'path'"))?;
            let parents = args["parents"].as_bool().unwrap_or(false);
            let (fs, rel) = r.route(path)?;
            let target = fs.resolve(rel)?;
            if parents { std::fs::create_dir_all(&target) }
            else       { std::fs::create_dir(&target) }
                .map_err(|e| anyhow!("fs_mkdir {:?}: {}", path, e))?;
            Ok(json!({}))
        });

        let r = self.clone();
        registry.register("fs_unlink", move |args| {
            let path = args["path"].as_str()
                .ok_or_else(|| anyhow!("fs_unlink: missing 'path'"))?;
            let (fs, rel) = r.route(path)?;
            let target = fs.resolve(rel)?;
            let md = std::fs::metadata(&target)
                .map_err(|e| anyhow!("fs_unlink {:?}: {}", path, e))?;
            if md.is_dir() { std::fs::remove_dir(&target) }
            else           { std::fs::remove_file(&target) }
                .map_err(|e| anyhow!("fs_unlink {:?}: {}", path, e))?;
            Ok(json!({}))
        });
    }
}

// ---------------------------------------------------------------------------
// Sandbox — the primary API (via `Sandbox::builder()`)
// ---------------------------------------------------------------------------

/// A Unikraft sandbox backed by Hyperlight's `MultiUseSandbox`.
///
/// Construct one with [`Sandbox::builder`]. Lifecycle:
///   1. `.build()` — creates the VM and runs guest init, takes a snapshot
///   2. [`Sandbox::restore`] — rewinds the VM to the post-init snapshot
///   3. [`Sandbox::call_run`] — runs the guest application
pub struct Sandbox {
    inner: MultiUseSandbox,
    /// Post-init snapshot for fast restore between calls.
    snapshot: Option<Arc<Snapshot>>,
    /// File mapping to re-register after snapshot restore.
    /// Snapshot restore unmaps all non-snapshot regions.
    file_mapping_path: Option<std::path::PathBuf>,
    file_mapping_base: u64,
}

/// Where the initrd comes from — either a file (zero-copy `map_file_cow`)
/// or an in-memory buffer (copied into snapshot memory).
enum InitrdSource {
    File(std::path::PathBuf),
    Bytes(Vec<u8>),
}

/// Fluent builder for [`Sandbox`]. Returned by [`Sandbox::builder`].
///
/// ```no_run
/// use hyperlight_unikraft::{Preopen, Sandbox};
///
/// let sandbox = Sandbox::builder("kernel.bin")
///     .initrd_file("app.cpio")
///     .args(["arg1", "arg2"])
///     .heap_size(16 << 20)
///     .preopen(Preopen::new("./work", "/data")?)
///     .tool("echo", |args| Ok(args))
///     .build()?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub struct SandboxBuilder {
    kernel: std::path::PathBuf,
    initrd: Option<InitrdSource>,
    args: Vec<String>,
    heap_size: Option<u64>,
    stack_size: Option<u64>,
    preopens: Vec<Preopen>,
    tools: ToolRegistry,
    has_tools: bool,
}

impl SandboxBuilder {
    /// The initrd CPIO archive, mapped zero-copy into guest memory.
    pub fn initrd_file<P: Into<std::path::PathBuf>>(mut self, path: P) -> Self {
        self.initrd = Some(InitrdSource::File(path.into()));
        self
    }

    /// An in-memory initrd buffer. Copied into snapshot memory.
    /// Prefer [`initrd_file`](Self::initrd_file) for anything non-trivial.
    pub fn initrd_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.initrd = Some(InitrdSource::Bytes(bytes));
        self
    }

    /// Application arguments, passed to the guest via the cmdline header.
    pub fn args<S, I>(mut self, args: I) -> Self
    where
        S: Into<String>,
        I: IntoIterator<Item = S>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Append a single argument. Repeatable.
    pub fn arg<S: Into<String>>(mut self, arg: S) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Guest heap size in bytes (default 512 MiB).
    pub fn heap_size(mut self, bytes: u64) -> Self {
        self.heap_size = Some(bytes);
        self
    }

    /// Guest stack size in bytes (default 8 MiB).
    pub fn stack_size(mut self, bytes: u64) -> Self {
        self.stack_size = Some(bytes);
        self
    }

    /// Expose a host directory to the guest. `lib/hostfs` mounts each
    /// `preopen.host_dir` at `preopen.guest_path`; FS tool handlers
    /// cover all of them and route by guest path prefix. Repeatable —
    /// call multiple times to expose several directories.
    pub fn preopen(mut self, preopen: Preopen) -> Self {
        self.preopens.push(preopen);
        self
    }

    /// Register a host function callable from the guest via `__dispatch`.
    pub fn tool<F>(mut self, name: &str, handler: F) -> Self
    where
        F: Fn(serde_json::Value) -> Result<serde_json::Value> + Send + Sync + 'static,
    {
        self.tools.register(name, handler);
        self.has_tools = true;
        self
    }

    /// Boot the VM, run init, and take a post-init snapshot.
    pub fn build(self) -> Result<Sandbox> {
        let config = VmConfig {
            heap_size: self.heap_size.unwrap_or(512 * 1024 * 1024),
            stack_size: self.stack_size.unwrap_or(8 * 1024 * 1024),
        };
        let tools = if self.has_tools { Some(self.tools) } else { None };
        match self.initrd {
            Some(InitrdSource::File(path)) => Sandbox::evolve_mapped(
                &self.kernel, Some(&path), &self.args, config, tools, &self.preopens,
            ),
            Some(InitrdSource::Bytes(bytes)) => Sandbox::evolve_inline(
                &self.kernel, Some(&bytes), &self.args, config, tools, &self.preopens,
            ),
            None => Sandbox::evolve_mapped(
                &self.kernel, None, &self.args, config, tools, &self.preopens,
            ),
        }
    }
}

impl Sandbox {
    /// Start building a sandbox. See [`SandboxBuilder`] for the chainable
    /// configuration methods.
    pub fn builder<P: Into<std::path::PathBuf>>(kernel: P) -> SandboxBuilder {
        SandboxBuilder {
            kernel: kernel.into(),
            initrd: None,
            args: Vec::new(),
            heap_size: None,
            stack_size: None,
            preopens: Vec::new(),
            tools: ToolRegistry::new(),
            has_tools: false,
        }
    }

    /// Low-level: boot with an in-memory initrd buffer. Prefer the builder.
    pub(crate) fn evolve_inline(
        kernel_path: &Path,
        initrd: Option<&[u8]>,
        app_args: &[String],
        config: VmConfig,
        tools: Option<ToolRegistry>,
        preopens: &[Preopen],
    ) -> Result<Self> {
        if !kernel_path.exists() {
            return Err(anyhow!("Kernel not found: {:?}", kernel_path));
        }

        let extended_initrd = prepend_cmdline_to_initrd(initrd, app_args, preopens);
        let env = GuestEnvironment::new(
            GuestBinary::FilePath(kernel_path.to_string_lossy().to_string()),
            extended_initrd.as_deref(),
        );

        let mut usbox = UninitializedSandbox::new(env, Some(config.sandbox_config()))?;

        let tools = build_tools(tools, preopens)?;

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

    /// Low-level: boot with a zero-copy mapped initrd file. Prefer the builder.
    pub(crate) fn evolve_mapped(
        kernel_path: &Path,
        initrd_path: Option<&Path>,
        app_args: &[String],
        config: VmConfig,
        tools: Option<ToolRegistry>,
        preopens: &[Preopen],
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

        // Build init_data with cmdline + preopens + mapped file size
        let cmdline_data = build_cmdline_initdata(app_args, mapped_size, preopens);
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

        let tools = build_tools(tools, preopens)?;

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

    /// Call a named guest function with typed parameters.
    ///
    /// Thin passthrough to [`MultiUseSandbox::call`] so callers can take
    /// advantage of Hyperlight's multi-function dispatch when the loaded
    /// ELF uses it (e.g. registering an `init` for one-time warm-up and
    /// a `run` for per-call work — see the FC-aware dispatch callback in
    /// plat/hyperlight/dispatch.c).
    ///
    /// Requires a prior `restore()` to reset guest state to the snapshot
    /// the caller wants to run against.
    pub fn call_named<Output, Args>(
        &mut self,
        func_name: &str,
        args: Args,
    ) -> Result<Output>
    where
        Output: hyperlight_host::func::SupportedReturnType,
        Args: hyperlight_host::func::ParameterTuple,
    {
        Ok(self.inner.call(func_name, args)?)
    }

    /// Take a new snapshot of the current guest state.
    ///
    /// Useful for the "snapshot after one-time warm-up" pattern: call
    /// `init` once to set up expensive state (e.g. `Py_Initialize` +
    /// heavy imports), then snapshot_now() here to capture the post-
    /// warm-up memory. Subsequent `restore()` calls will return the VM
    /// to this warm state, so per-call work skips the warm-up entirely.
    ///
    /// After this call, future `restore()` calls rewind to the *new*
    /// snapshot rather than the post-evolve one.
    pub fn snapshot_now(&mut self) -> Result<()> {
        let snap = self.inner.snapshot()?;
        self.snapshot = Some(snap);
        Ok(())
    }

    /// Persist the current post-evolve (or post-`snapshot_now`) snapshot
    /// to disk so a later process can skip evolve + init and go straight
    /// to `call`. Uses hyperlight's `Snapshot::to_file` — the file
    /// format and cross-platform mmap load are documented in
    /// hyperlight/docs/snapshot-file-implementation-plan.md.
    pub fn save_snapshot<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let snap = self.snapshot.as_ref().ok_or_else(|| {
            anyhow!("no snapshot present; build() or snapshot_now() first")
        })?;
        snap.to_file(path.as_ref())?;
        Ok(())
    }

    /// Load a previously-persisted snapshot from disk and create a
    /// `Sandbox` directly from it, bypassing the entire evolve path.
    /// Every subsequent `call*` runs against the snapshot's post-warmup
    /// state; `restore()` rewinds to it.
    ///
    /// This is the `pyhl run` fast path: `pyhl setup` persists the
    /// warm-Python snapshot once, and every `pyhl run` instantiates
    /// straight from it — no kernel boot, no Py_Initialize.
    ///
    /// Uses `Snapshot::from_file_unchecked`, which skips the SHA-256
    /// verification over the file. We trust snapshots written by our
    /// own `save_snapshot()` earlier in the same process family (the
    /// pyhl install dir), and the hash verify alone costs ~500ms on
    /// a 2.5 GB snapshot — enough to double the whole `pyhl run` wall
    /// time on simple scripts.
    pub fn from_snapshot_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::from_snapshot_file_with(path, &[])
    }

    /// Load a previously-persisted snapshot and register a
    /// preopen-backed `__dispatch` host function on the loaded sandbox,
    /// so guest code that does file I/O through `lib/hostfs` has
    /// working RPC paths.
    ///
    /// The snapshot must have been taken while the guest had hostfs
    /// mounted at each preopen's guest_path (i.e. `pyhl setup` was
    /// invoked with the same guest_paths). At run time only the
    /// `host_dir` side is remapped — the guest-side mount point is
    /// fixed at setup time because it lives in the snapshot's memory
    /// image.
    pub fn from_snapshot_file_with<P: AsRef<Path>>(
        path: P,
        preopens: &[Preopen],
    ) -> Result<Self> {
        let loaded = Snapshot::from_file_unchecked(path.as_ref())?;
        let arc = Arc::new(loaded);
        let mut inner = MultiUseSandbox::from_snapshot(arc.clone())?;

        // Wire up the fs_* tool handlers against the caller's preopens.
        // The snapshot was warmed up with hostfs already mounted, so the
        // guest will route fs_* calls through __dispatch → the FsRouter
        // we install here.
        if !preopens.is_empty() {
            if let Some(tools) = build_tools(None, preopens)? {
                let tools = Arc::new(tools);
                let tools_ref = tools.clone();
                inner.register_host_function(
                    "__dispatch",
                    move |payload: Vec<u8>| -> Vec<u8> {
                        tools_ref.dispatch(&payload)
                    },
                )?;
            }
        }

        Ok(Self {
            inner,
            snapshot: Some(arc),
            file_mapping_path: None,
            file_mapping_base: 0,
        })
    }
}

// ---------------------------------------------------------------------------
// Convenience: run_vm (single-shot execution)
// ---------------------------------------------------------------------------

/// Run a Unikraft kernel to completion (single-shot). Thin shim over
/// [`Sandbox::builder`] for callers that don't need the full fluent API.
pub fn run_vm(
    kernel_path: &Path,
    initrd: Option<&[u8]>,
    app_args: &[String],
    config: VmConfig,
) -> Result<()> {
    let _ = Sandbox::evolve_inline(kernel_path, initrd, app_args, config, None, &[])?;
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
    let _ = Sandbox::evolve_inline(
        kernel_path, initrd, app_args, config, Some(tools), &[],
    )?;
    Ok(())
}

/// Run a Unikraft kernel with preopened host directories exposed via
/// `lib/hostfs`. Sandbox escape attempts are rejected host-side.
pub fn run_vm_with_preopens(
    kernel_path: &Path,
    initrd: Option<&[u8]>,
    app_args: &[String],
    config: VmConfig,
    preopens: &[Preopen],
) -> Result<()> {
    let _ = Sandbox::evolve_inline(
        kernel_path, initrd, app_args, config, None, preopens,
    )?;
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
    let mut sandbox =
        Sandbox::evolve_inline(kernel_path, initrd, app_args, config, None, &[])?;
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
    fn preopen_parse_defaults_guest_to_host() {
        let dir = tmpdir("po1");
        let p = Preopen::parse_cli(dir.to_str().unwrap()).unwrap();
        assert_eq!(p.guest_path, "/host");
        assert_eq!(p.host_dir, std::fs::canonicalize(&dir).unwrap());
    }

    #[test]
    fn preopen_parse_accepts_custom_guest_path() {
        let dir = tmpdir("po2");
        let spec = format!("{}:/data", dir.display());
        let p = Preopen::parse_cli(&spec).unwrap();
        assert_eq!(p.guest_path, "/data");
    }

    #[test]
    fn preopen_rejects_reserved_guest_path() {
        let dir = tmpdir("po3");
        for reserved in &["/", "/bin", "/dev", "/proc", "/sys", "/usr", "/bin/foo"] {
            let err = Preopen::new(&dir, *reserved).unwrap_err().to_string();
            assert!(err.contains("reserved"), "{reserved}: {err}");
        }
    }

    #[test]
    fn preopen_rejects_relative_guest_path() {
        let dir = tmpdir("po4");
        let err = Preopen::new(&dir, "relative").unwrap_err().to_string();
        assert!(err.contains("absolute"), "{err}");
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn initdata_carries_mount_tlv_when_preopens_set() {
        let root_a = tmpdir("mnt-a");
        let root_b = tmpdir("mnt-b");
        let preopens = vec![
            Preopen::new(&root_a, "/data").unwrap(),
            Preopen::new(&root_b, "/logs").unwrap(),
        ];
        let buf = build_cmdline_initdata(
            &["/hello".to_string()],
            0,
            &preopens,
        )
        .expect("initdata");
        assert!(buf.starts_with(CMDLINE_MAGIC), "cmdline magic missing");
        let off = find_subslice(&buf, MOUNT_MAGIC).expect("mount magic missing");
        let count_off = off + MOUNT_MAGIC.len();
        let count = u32::from_le_bytes(
            buf[count_off..count_off + 4].try_into().unwrap(),
        );
        assert_eq!(count, 2);
        // First path is /data, second is /logs.
        let mut p = count_off + 4;
        for expected in ["/data", "/logs"] {
            let len = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap()) as usize;
            assert_eq!(&buf[p + 4..p + 4 + len], expected.as_bytes());
            assert_eq!(buf[p + 4 + len], 0);
            p += 4 + len + 1;
        }
    }

    #[test]
    fn initdata_omits_mount_tlv_when_no_preopens() {
        let buf = build_cmdline_initdata(&["/hello".to_string()], 0, &[])
            .expect("initdata");
        assert!(buf.starts_with(CMDLINE_MAGIC));
        assert!(find_subslice(&buf, MOUNT_MAGIC).is_none(),
                "no mount TLV expected");
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
