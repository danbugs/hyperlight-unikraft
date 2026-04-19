//! pydriver-test — drives the hl_pydriver guest through the
//! multi-function dispatch lifecycle and measures per-call latency.
//!
//! Expected usage:
//!   pydriver-test <kernel> <initrd.cpio> [--code CODE]...
//!
//! Sequence:
//!   build()                         -> evolve timing
//!   call_named("init", ())          -> one-time Py_Initialize + imports
//!   snapshot_now()                  -> capture warm state
//!   for each --code string:
//!       restore()
//!       call_named("run", code)     -> measured
//!
//! Without any --code args, runs one canonical smoke test
//! ("import numpy, pandas; print(...)").

use anyhow::Result;
use hyperlight_unikraft::Sandbox;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<()> {
    let mut kernel: Option<PathBuf> = None;
    let mut initrd: Option<PathBuf> = None;
    let mut codes: Vec<String> = Vec::new();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--code" {
            i += 1;
            codes.push(args[i].clone());
        } else if kernel.is_none() {
            kernel = Some(PathBuf::from(a));
        } else if initrd.is_none() {
            initrd = Some(PathBuf::from(a));
        } else {
            eprintln!("unexpected arg: {}", a);
            std::process::exit(2);
        }
        i += 1;
    }
    let kernel = kernel.ok_or_else(|| anyhow::anyhow!("missing <kernel>"))?;
    let initrd = initrd.ok_or_else(|| anyhow::anyhow!("missing <initrd>"))?;

    if codes.is_empty() {
        codes.push(
            "import numpy, pandas\nprint(f'numpy={numpy.__version__} pandas={pandas.__version__}')"
                .to_string(),
        );
    }

    let t0 = Instant::now();
    let mut sandbox = Sandbox::builder(&kernel)
        .initrd_file(&initrd)
        .heap_size(1024 * 1024 * 1024)
        .build()?;
    let evolve_ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("[timing] evolve={:.1}ms", evolve_ms);

    let t_init = Instant::now();
    sandbox.restore()?;
    let _: () = sandbox.call_named("init", ())?;
    let init_ms = t_init.elapsed().as_secs_f64() * 1000.0;
    eprintln!("[timing] init={:.1}ms (imports happen here)", init_ms);

    let no_snapshot = std::env::var_os("NO_SNAPSHOT").is_some();

    if !no_snapshot {
        let t_snap = Instant::now();
        sandbox.snapshot_now()?;
        let snap_ms = t_snap.elapsed().as_secs_f64() * 1000.0;
        eprintln!("[timing] snapshot_now={:.1}ms", snap_ms);
    } else {
        eprintln!("[timing] skipping snapshot_now (NO_SNAPSHOT set)");
    }

    for (idx, code) in codes.iter().enumerate() {
        let mut restore_ms = 0.0;
        if !no_snapshot {
            let t_restore = Instant::now();
            sandbox.restore()?;
            restore_ms = t_restore.elapsed().as_secs_f64() * 1000.0;
        }

        let t_call = Instant::now();
        let _: () = sandbox.call_named("run", code.clone())?;
        let call_ms = t_call.elapsed().as_secs_f64() * 1000.0;

        eprintln!(
            "[run {}/{}] restore={:.1}ms call={:.1}ms",
            idx + 1,
            codes.len(),
            restore_ms,
            call_ms,
        );
    }

    Ok(())
}
