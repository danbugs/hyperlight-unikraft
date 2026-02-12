//! hyperlight-unikraft library for embedding Unikraft VMs
//!
//! This library provides functionality to run Unikraft kernels on Hyperlight.

use anyhow::{anyhow, Result};
use hyperlight_host::sandbox::uninitialized::GuestEnvironment;
use hyperlight_host::sandbox::SandboxConfiguration;
use hyperlight_host::{GuestBinary, UninitializedSandbox};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Magic header for cmdline embedded in initrd: "HLCMDLN\0"
const CMDLINE_MAGIC: &[u8; 8] = b"HLCMDLN\0";

/// Page size for alignment (must match Unikraft's PAGE_SIZE)
const PAGE_SIZE: usize = 4096;

/// Configuration for running a Unikraft VM
pub struct VmConfig {
    /// Heap size in bytes
    pub heap_size: u64,
    /// Stack size in bytes
    pub stack_size: u64,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            heap_size: 512 * 1024 * 1024,  // 512Mi
            stack_size: 8 * 1024 * 1024,   // 8Mi
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
}

/// Parse memory size string (e.g., "512Mi", "1Gi") into bytes
pub fn parse_memory(mem_str: &str) -> Result<u64> {
    let mem_str = mem_str.trim();

    if mem_str.ends_with("Gi") {
        let val: u64 = mem_str.trim_end_matches("Gi").parse()?;
        Ok(val * 1024 * 1024 * 1024)
    } else if mem_str.ends_with("Mi") {
        let val: u64 = mem_str.trim_end_matches("Mi").parse()?;
        Ok(val * 1024 * 1024)
    } else if mem_str.ends_with("Ki") {
        let val: u64 = mem_str.trim_end_matches("Ki").parse()?;
        Ok(val * 1024)
    } else if mem_str.ends_with("G") {
        let val: u64 = mem_str.trim_end_matches("G").parse()?;
        Ok(val * 1000 * 1000 * 1000)
    } else if mem_str.ends_with("M") {
        let val: u64 = mem_str.trim_end_matches("M").parse()?;
        Ok(val * 1000 * 1000)
    } else if mem_str.ends_with("K") {
        let val: u64 = mem_str.trim_end_matches("K").parse()?;
        Ok(val * 1000)
    } else {
        mem_str
            .parse()
            .map_err(|e| anyhow!("Invalid memory format: {}", e))
    }
}

/// Create an extended initrd with cmdline prepended
///
/// Format:
/// | Magic (8 bytes): "HLCMDLN\0" |
/// | Cmdline length (4 bytes LE)  |
/// | Cmdline data (null-term)     |
/// | Padding to PAGE_SIZE boundary|
/// | Original initrd...           |
pub fn prepend_cmdline_to_initrd(initrd: Option<&[u8]>, app_args: &[String]) -> Option<Vec<u8>> {
    let cmdline = if app_args.is_empty() {
        String::new()
    } else {
        app_args.join(" ")
    };

    if cmdline.is_empty() && initrd.is_none() {
        return None;
    }

    if cmdline.is_empty() {
        return initrd.map(|d| d.to_vec());
    }

    let cmdline_bytes = cmdline.as_bytes();
    let cmdline_len = cmdline_bytes.len() as u32;

    let header_size = 8 + 4 + cmdline_len as usize + 1;
    let padded_header_size = (header_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let padding = padded_header_size - header_size;

    let initrd_len = initrd.map(|d| d.len()).unwrap_or(0);
    let mut extended = Vec::with_capacity(padded_header_size + initrd_len);

    extended.extend_from_slice(CMDLINE_MAGIC);
    extended.extend_from_slice(&cmdline_len.to_le_bytes());
    extended.extend_from_slice(cmdline_bytes);
    extended.push(0);
    extended.extend(std::iter::repeat(0).take(padding));

    if let Some(data) = initrd {
        extended.extend_from_slice(data);
    }

    Some(extended)
}

/// Run a Unikraft kernel with the given configuration
///
/// Returns Ok(()) on successful completion, or an error if the VM fails to start.
/// Note: Unikernels typically exit via HLT which returns an error - this is expected.
pub fn run_vm(
    kernel_path: &Path,
    initrd: Option<&[u8]>,
    app_args: &[String],
    config: VmConfig,
) -> Result<()> {
    if !kernel_path.exists() {
        return Err(anyhow!("Kernel file not found: {:?}", kernel_path));
    }

    let t0 = std::time::Instant::now();

    let mut sandbox_config = SandboxConfiguration::default();
    sandbox_config.set_heap_size(config.heap_size);
    sandbox_config.set_stack_size(config.stack_size);

    let extended_initrd = prepend_cmdline_to_initrd(initrd, app_args);
    let prepend_time = t0.elapsed();

    let t1 = std::time::Instant::now();
    let kernel_path_str = kernel_path.to_string_lossy().to_string();
    let env = GuestEnvironment::new(
        GuestBinary::FilePath(kernel_path_str),
        extended_initrd.as_deref(),
    );

    let sandbox = UninitializedSandbox::new(env, Some(sandbox_config))?;
    let sandbox_time = t1.elapsed();

    let t2 = std::time::Instant::now();
    // Run the kernel - unikernels exit via HLT which returns an error
    match sandbox.evolve() {
        Ok(_) => {}
        Err(_) => {} // HLT is expected for unikernels
    }
    let evolve_time = t2.elapsed();

    eprintln!(
        "[timing] prepend={:.1}ms sandbox_new={:.1}ms evolve={:.1}ms",
        prepend_time.as_secs_f64() * 1000.0,
        sandbox_time.as_secs_f64() * 1000.0,
        evolve_time.as_secs_f64() * 1000.0,
    );

    Ok(())
}

/// Result of running a VM with captured output
pub struct VmOutput {
    /// Captured stderr output (Hyperlight's DebugPrint)
    pub output: String,
    /// Time spent in sandbox.evolve() (actual VM execution)
    pub evolve_time: std::time::Duration,
    /// Time spent creating the sandbox
    pub setup_time: std::time::Duration,
}

/// Run a Unikraft kernel and capture its output (stderr)
///
/// Hyperlight's DebugPrint outputs to stderr via eprint!.
/// This function captures that output and returns it along with timing info.
pub fn run_vm_capture_output(
    kernel_path: &Path,
    initrd: Option<&[u8]>,
    app_args: &[String],
    config: VmConfig,
) -> Result<VmOutput> {
    if !kernel_path.exists() {
        return Err(anyhow!("Kernel file not found: {:?}", kernel_path));
    }

    let setup_start = std::time::Instant::now();

    let mut sandbox_config = SandboxConfiguration::default();
    sandbox_config.set_heap_size(config.heap_size);
    sandbox_config.set_stack_size(config.stack_size);

    let extended_initrd = prepend_cmdline_to_initrd(initrd, app_args);

    let kernel_path_str = kernel_path.to_string_lossy().to_string();
    let env = GuestEnvironment::new(
        GuestBinary::FilePath(kernel_path_str),
        extended_initrd.as_deref(),
    );

    let sandbox = UninitializedSandbox::new(env, Some(sandbox_config))?;

    let setup_time = setup_start.elapsed();

    // Capture stderr by redirecting it to a pipe
    let (read_fd, write_fd) = nix::unistd::pipe()?;

    // Save original stderr
    let original_stderr = nix::unistd::dup(2)?;

    // Redirect stderr to our pipe
    nix::unistd::dup2(write_fd.as_raw_fd(), 2)?;

    // Run the kernel
    let evolve_start = std::time::Instant::now();
    let result = sandbox.evolve();
    let evolve_time = evolve_start.elapsed();

    // Flush stderr and restore original
    std::io::stderr().flush().ok();
    nix::unistd::dup2(original_stderr.as_raw_fd(), 2)?;
    let _ = original_stderr;
    let _ = write_fd;

    // Read captured output
    let mut captured = String::new();
    let mut reader = std::fs::File::from(read_fd);
    
    // Set non-blocking to avoid hanging if no output
    let flags = nix::fcntl::fcntl(reader.as_raw_fd(), nix::fcntl::FcntlArg::F_GETFL)?;
    nix::fcntl::fcntl(
        reader.as_raw_fd(),
        nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK),
    )?;
    
    reader.read_to_string(&mut captured).ok();

    // We don't care about the result - HLT is expected
    match result {
        Ok(_) | Err(_) => Ok(VmOutput {
            output: captured,
            evolve_time,
            setup_time,
        }),
    }
}
