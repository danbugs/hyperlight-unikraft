//! hyperlight-unikraft: run Unikraft unikernels on the Hyperlight VMM
//!
//! ## Usage
//!
//! ```bash
//! hyperlight-unikraft <kernel> [--initrd <cpio>] [--memory <size>] [-- <app-args>]
//! ```

use anyhow::Result;
use clap::Parser;
use hyperlight_unikraft::{parse_memory, Sandbox, ToolRegistry, VmConfig};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "hyperlight-unikraft", version, about = "Run Unikraft unikernels on Hyperlight")]
struct Args {
    /// Path to the Unikraft kernel binary
    kernel: PathBuf,

    /// Path to initrd/rootfs CPIO archive
    #[arg(long)]
    initrd: Option<PathBuf>,

    /// Memory allocation (e.g., 256Mi, 512Mi, 1Gi)
    #[arg(long, short = 'm', default_value = "512Mi")]
    memory: String,

    /// Stack size (e.g., 8Mi)
    #[arg(long, default_value = "8Mi")]
    stack: String,

    /// Quiet mode — suppress host-side status messages
    #[arg(long, short = 'q')]
    quiet: bool,

    /// Enable tool dispatch via __dispatch host function
    #[arg(long)]
    enable_tools: bool,

    /// Run the application N additional times via snapshot/restore + call.
    /// The first run always happens. --repeat=2 means 3 total runs.
    #[arg(long, default_value = "0")]
    repeat: u32,

    /// Application arguments (passed after --)
    #[arg(last = true)]
    app_args: Vec<String>,
}

fn main() -> Result<()> {
    let t0 = std::time::Instant::now();
    let args = Args::parse();

    let heap_size = parse_memory(&args.memory)?;
    let stack_size = parse_memory(&args.stack)?;

    if !args.quiet {
        eprintln!("hyperlight-unikraft v{}", env!("CARGO_PKG_VERSION"));
        eprintln!("Kernel: {:?}", args.kernel);
        if let Some(ref p) = args.initrd {
            eprintln!("Initrd: {:?}", p);
        }
        eprintln!("Memory: {heap_size} B, Stack: {stack_size} B");
    }

    let config = VmConfig::default()
        .with_heap_size(heap_size)
        .with_stack_size(stack_size);

    let tools = if args.enable_tools {
        let mut t = ToolRegistry::new();
        t.register("echo", |a| Ok(a));
        Some(t)
    } else {
        None
    };

    // Phase 1: evolve — boots kernel, loads ELF, signals ready
    // Use map_file_cow for zero-copy initrd mapping
    let mut sandbox = Sandbox::new_with_file_initrd(
        &args.kernel,
        args.initrd.as_deref(),
        &args.app_args,
        config,
        tools,
    )?;
    let evolve_time = t0.elapsed();

    // Phase 2: restore + call — runs the application
    let total_runs = 1 + args.repeat;
    for i in 0..total_runs {
        let t_restore = std::time::Instant::now();
        sandbox.restore()?;
        let restore_time = t_restore.elapsed();

        let t_call = std::time::Instant::now();
        sandbox.call_run()?;
        let call_time = t_call.elapsed();

        if !args.quiet || args.repeat > 0 {
            eprintln!(
                "[run {}/{}] restore={:.1}ms call={:.1}ms",
                i + 1,
                total_runs,
                restore_time.as_secs_f64() * 1000.0,
                call_time.as_secs_f64() * 1000.0,
            );
        }
    }

    eprintln!(
        "[timing] evolve={:.1}ms total={:.1}ms",
        evolve_time.as_secs_f64() * 1000.0,
        t0.elapsed().as_secs_f64() * 1000.0,
    );
    Ok(())
}
