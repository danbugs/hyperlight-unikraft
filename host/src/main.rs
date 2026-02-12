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
    let total_start = std::time::Instant::now();
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

    let t0 = std::time::Instant::now();
    let initrd_file = if let Some(ref path) = args.initrd {
        Some(std::fs::File::open(path)?)
    } else {
        None
    };
    let initrd_mmap = if let Some(ref file) = initrd_file {
        Some(unsafe { memmap2::Mmap::map(file)? })
    } else {
        None
    };
    let read_time = t0.elapsed();

    let config = VmConfig::default()
        .with_heap_size(heap_size)
        .with_stack_size(stack_size);

    let t1 = std::time::Instant::now();
    run_vm(
        &args.kernel,
        initrd_mmap.as_deref(),
        &args.app_args,
        config,
    )?;
    let vm_time = t1.elapsed();

    let total_time = total_start.elapsed();

    if !args.quiet {
        eprintln!("Kernel completed");
    }

    eprintln!(
        "[timing] initrd_read={:.1}ms vm_total={:.1}ms total={:.1}ms initrd_size={}",
        read_time.as_secs_f64() * 1000.0,
        vm_time.as_secs_f64() * 1000.0,
        total_time.as_secs_f64() * 1000.0,
        initrd_mmap.as_ref().map(|d| d.len()).unwrap_or(0),
    );

    Ok(())
}
