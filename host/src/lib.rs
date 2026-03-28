//! hyperlight-unikraft: run Unikraft kernels on Hyperlight
//!
//! Provides a `Sandbox` wrapper around Hyperlight's `MultiUseSandbox` that
//! manages the kernel lifecycle: create → evolve (init) → snapshot → call.

pub mod ffi;

use anyhow::{anyhow, Result};
use hyperlight_host::func::Registerable;
use hyperlight_host::sandbox::uninitialized::GuestEnvironment;
use hyperlight_host::sandbox::SandboxConfiguration;
use hyperlight_host::sandbox::snapshot::Snapshot;
use hyperlight_host::{GuestBinary, MultiUseSandbox, UninitializedSandbox};
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Magic header for cmdline embedded in initrd: "HLCMDLN\0"
const CMDLINE_MAGIC: &[u8; 8] = b"HLCMDLN\0";

const PAGE_SIZE: usize = 4096;

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

        // Register tool dispatch host function if tools are provided
        if let Some(tools) = tools {
            let tools = Arc::new(tools);
            let tools_ref = tools.clone();
            usbox.register_host_function(
                "__dispatch",
                move |payload: Vec<u8>| -> Vec<u8> { tools_ref.dispatch(&payload) },
            )?;
        }

        // Evolve runs the guest.  The unikernel boots, runs the application,
        // then signals readiness via outb(108) with the dispatch function
        // address in RAX.  This satisfies Hyperlight's init protocol and
        // returns a MultiUseSandbox ready for call/snapshot/restore.
        let mut inner = usbox.evolve()?;

        // Take a snapshot of the post-init state for fast restore.
        // This captures the fully-booted kernel + completed application.
        // Future: snapshot should be taken BEFORE the application runs
        // (requires deferred execution in the elfloader).
        let snapshot = inner.snapshot().ok();

        Ok(Self { inner, snapshot })
    }

    /// Restore the sandbox to its post-init snapshot.
    ///
    /// This is a fast operation (host-level CoW via mmap) that resets all
    /// guest memory to the state captured after init.
    pub fn restore(&mut self) -> Result<()> {
        if let Some(ref snap) = self.snapshot {
            self.inner.restore(snap.clone())?;
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
    let _ = Sandbox::new(kernel_path, initrd, app_args, config, None)?;
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
    let _ = Sandbox::new(kernel_path, initrd, app_args, config, Some(tools))?;
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
    let mut sandbox = Sandbox::new(kernel_path, initrd, app_args, config, None)?;
    let setup_time = setup_start.elapsed();

    // Redirect stderr to a temp file before the call phase
    let capture_file = std::env::temp_dir().join(format!("hl-capture-{}", std::process::id()));
    let capture_fd = {
        use std::os::fd::IntoRawFd;
        std::fs::File::create(&capture_file)?.into_raw_fd()
    };
    let original_stderr = nix::unistd::dup(2)?;
    nix::unistd::dup2(capture_fd, 2)?;
    nix::unistd::close(capture_fd)?;

    // Phase 2: restore + call — application runs and produces output
    let evolve_start = std::time::Instant::now();
    sandbox.restore()?;
    let call_result = sandbox.call_run();
    let evolve_time = evolve_start.elapsed();

    // Restore stderr
    nix::unistd::dup2(original_stderr.as_raw_fd(), 2)?;

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
