//! hyperlight-unikraft: Embedded Hyperlight host for Unikraft unikernels
//!
//! This runs Unikraft kernels built for the Hyperlight platform. The kernel
//! uses the ELF loader to dynamically load and execute Linux binaries from
//! an initrd (CPIO archive).
//!
//! ## Usage
//!
//! ```bash
//! hyperlight-unikraft <kernel> [--initrd <cpio>] [--memory <size>] [-- <app-args>]
//! ```
//!
//! ## Example
//!
//! ```bash
//! # Run Python with a script
//! hyperlight-unikraft kernel.elf --initrd python-initrd.cpio --memory 256Mi -- /hello.py
//!
//! # Run Node.js
//! hyperlight-unikraft kernel.elf --initrd node-initrd.cpio --memory 256Mi -- /app/hello.js
//! ```

use anyhow::{anyhow, Result};
use clap::Parser;
use hyperlight_host::sandbox::uninitialized::GuestEnvironment;
use hyperlight_host::sandbox::SandboxConfiguration;
use hyperlight_host::{GuestBinary, UninitializedSandbox};
use std::path::PathBuf;

/// Magic header for cmdline embedded in initrd: "HLCMDLN\0"
const CMDLINE_MAGIC: &[u8; 8] = b"HLCMDLN\0";

/// Page size for alignment (must match Unikraft's PAGE_SIZE)
const PAGE_SIZE: usize = 4096;

/// Create an extended initrd with cmdline prepended
///
/// Format:
/// | Magic (8 bytes): "HLCMDLN\0" |
/// | Cmdline length (4 bytes LE)  |
/// | Cmdline data (null-term)     |
/// | Padding to PAGE_SIZE boundary|
/// | Original initrd...           |
///
/// The header is padded to page alignment so that the initrd
/// starts at a page-aligned offset (required by Unikraft).
fn prepend_cmdline_to_initrd(initrd: Option<&[u8]>, app_args: &[String]) -> Option<Vec<u8>> {
    // Build cmdline string: "-- arg1 arg2 ..."
    let cmdline = if app_args.is_empty() {
        String::new()
    } else {
        format!("-- {}", app_args.join(" "))
    };

    // If no cmdline and no initrd, return None
    if cmdline.is_empty() && initrd.is_none() {
        return None;
    }

    // If no cmdline but have initrd, return initrd as-is
    if cmdline.is_empty() {
        return initrd.map(|d| d.to_vec());
    }

    // Build extended initrd with cmdline header
    let cmdline_bytes = cmdline.as_bytes();
    let cmdline_len = cmdline_bytes.len() as u32;

    // Calculate total header size (aligned to PAGE_SIZE for Unikraft)
    let header_size = 8 + 4 + cmdline_len as usize + 1; // magic + len + cmdline + null
    let padded_header_size = (header_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1); // align to page
    let padding = padded_header_size - header_size;

    let initrd_len = initrd.map(|d| d.len()).unwrap_or(0);
    let mut extended = Vec::with_capacity(padded_header_size + initrd_len);

    // Write magic
    extended.extend_from_slice(CMDLINE_MAGIC);

    // Write cmdline length (4 bytes LE)
    extended.extend_from_slice(&cmdline_len.to_le_bytes());

    // Write cmdline data
    extended.extend_from_slice(cmdline_bytes);
    extended.push(0); // null terminator

    // Add padding to page boundary
    extended.extend(std::iter::repeat(0).take(padding));

    // Append original initrd
    if let Some(data) = initrd {
        extended.extend_from_slice(data);
    }

    Some(extended)
}

/// Run Unikraft unikernels on Hyperlight
#[derive(Parser, Debug)]
#[command(name = "hyperlight-unikraft")]
#[command(about = "Run Unikraft unikernels on the Hyperlight VMM")]
#[command(version)]
struct Args {
    /// Path to the Unikraft kernel binary
    kernel: PathBuf,

    /// Path to initrd/rootfs CPIO archive (contains the application and libs)
    #[arg(long)]
    initrd: Option<PathBuf>,

    /// Memory allocation (e.g., 256Mi, 512Mi, 1Gi)
    #[arg(long, short = 'm', default_value = "512Mi")]
    memory: String,

    /// Stack size (e.g., 8Mi)
    #[arg(long, default_value = "8Mi")]
    stack: String,

    /// Quiet mode - suppress kernel output
    #[arg(long, short = 'q')]
    quiet: bool,

    /// Application arguments (passed after --)
    #[arg(last = true)]
    app_args: Vec<String>,
}

fn parse_memory(mem_str: &str) -> Result<u64> {
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

fn main() -> Result<()> {
    let args = Args::parse();

    if !args.kernel.exists() {
        return Err(anyhow!("Kernel file not found: {:?}", args.kernel));
    }

    let heap_size = parse_memory(&args.memory)?;
    let stack_size = parse_memory(&args.stack)?;

    if !args.quiet {
        eprintln!("Loading kernel: {:?}", args.kernel);
        if let Some(ref initrd) = args.initrd {
            eprintln!("Loading initrd: {:?}", initrd);
        }
        eprintln!("Memory: {} bytes, Stack: {} bytes", heap_size, stack_size);
        if !args.app_args.is_empty() {
            eprintln!("App args: {:?}", args.app_args);
        }
    }

    // Create configuration
    let mut config = SandboxConfiguration::default();
    config.set_heap_size(heap_size);
    config.set_stack_size(stack_size);

    // Load initrd if provided
    let initrd_data = if let Some(ref path) = args.initrd {
        if !path.exists() {
            return Err(anyhow!("Initrd file not found: {:?}", path));
        }
        Some(std::fs::read(path)?)
    } else {
        None
    };

    // Prepend cmdline args to initrd with magic header
    // This allows Unikraft to extract app args from the init_data
    let extended_initrd = prepend_cmdline_to_initrd(initrd_data.as_deref(), &args.app_args);

    // Create guest environment
    let kernel_path_str = args.kernel.to_string_lossy().to_string();
    let env = GuestEnvironment::new(GuestBinary::FilePath(kernel_path_str), extended_initrd.as_deref());

    // Create sandbox
    let sandbox = UninitializedSandbox::new(env, Some(config))?;

    if !args.quiet {
        eprintln!("Starting kernel...");
    }

    // Run the kernel
    match sandbox.evolve() {
        Ok(_) => {
            if !args.quiet {
                eprintln!("Kernel completed");
            }
        }
        Err(e) => {
            // Kernel exited (HLT or abort) - this is expected for unikernels
            if !args.quiet {
                eprintln!("Kernel exited: {:?}", e);
            }
        }
    }

    Ok(())
}
