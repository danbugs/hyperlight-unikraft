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

use anyhow::Result;
use clap::Parser;
use hyperlight_unikraft::{parse_memory, run_vm, VmConfig};
use std::path::PathBuf;

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

fn main() -> Result<()> {
    let args = Args::parse();

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
        eprintln!("Starting kernel...");
    }

    let initrd_data = if let Some(ref path) = args.initrd {
        Some(std::fs::read(path)?)
    } else {
        None
    };

    let config = VmConfig::default()
        .with_heap_size(heap_size)
        .with_stack_size(stack_size);

    run_vm(
        &args.kernel,
        initrd_data.as_deref(),
        &args.app_args,
        config,
    )?;

    if !args.quiet {
        eprintln!("Kernel completed");
    }

    Ok(())
}
