//! hyperlight-unikraft: run Unikraft unikernels on the Hyperlight VMM
//!
//! ## Usage
//!
//! ```bash
//! hyperlight-unikraft <kernel> [--initrd <cpio>] [--memory <size>] [-- <app-args>]
//! ```

use anyhow::Result;
use clap::Parser;
use hyperlight_unikraft::{parse_memory, run_vm, run_vm_with_tools, ToolRegistry, VmConfig};
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

    let initrd_mmap = args.initrd.as_ref().map(|p| {
        let f = std::fs::File::open(p).expect("open initrd");
        unsafe { memmap2::Mmap::map(&f).expect("mmap initrd") }
    });

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

    if tools.is_some() {
        run_vm_with_tools(
            &args.kernel,
            initrd_mmap.as_deref(),
            &args.app_args,
            config,
            tools.unwrap(),
        )?;
    } else {
        run_vm(
            &args.kernel,
            initrd_mmap.as_deref(),
            &args.app_args,
            config,
        )?;
    }

    eprintln!("[timing] total={:.1}ms", t0.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}
