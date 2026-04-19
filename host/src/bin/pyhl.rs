//! pyhl — run Python against the prebuilt python-agent-driver image.
//!
//! Assumes the `python-agent-driver` example has been built
//! (`just build` + `just rootfs` in examples/python-agent-driver/).
//!
//! Usage:
//!   pyhl <script.py> [--repeat N]    run a Python file
//!   pyhl -c '<code>'  [--repeat N]   run an inline snippet
//!   pyhl --help
//!
//! Override the default image with env vars:
//!   PYHL_KERNEL   path to the Unikraft kernel ELF
//!   PYHL_INITRD   path to the driver's CPIO rootfs
//!
//! First call pays the ~3.5s Py_Initialize + warm-import cost; every
//! subsequent call goes through the in-guest v2 dispatch callback and
//! runs in single-digit ms.

use anyhow::{anyhow, Context, Result};
use hyperlight_unikraft::Sandbox;
use std::path::{Path, PathBuf};
use std::time::Instant;

const USAGE: &str = "\
pyhl — run Python on hyperlight-unikraft

USAGE:
    pyhl <script.py> [--repeat N]
    pyhl -c '<code>'  [--repeat N]

OPTIONS:
    --repeat N       Run the code N additional times after the first
                     (useful to see warm-path timings).
    -h, --help       Show this help.

ENV:
    PYHL_KERNEL      Override the kernel path.
    PYHL_INITRD      Override the initrd CPIO path.

The defaults point at examples/python-agent-driver/ in the source tree.
";

/// Repo-relative path from this crate's manifest dir to the driver
/// artifacts produced by `examples/python-agent-driver`.
fn default_kernel() -> PathBuf {
    if let Ok(p) = std::env::var("PYHL_KERNEL") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("examples/python-agent-driver/.unikraft/build")
        .join("python-agent-driver-hyperlight_hyperlight-x86_64")
}

fn default_initrd() -> PathBuf {
    if let Ok(p) = std::env::var("PYHL_INITRD") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("examples/python-agent-driver/python-agent-driver-initrd.cpio")
}

fn require_exists(label: &str, path: &Path, hint: &str) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!(
            "{label} not found at {path}\n  hint: {hint}",
            label = label,
            path = path.display(),
            hint = hint,
        ));
    }
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).peekable();

    let mut code: Option<String> = None;
    let mut script_path: Option<PathBuf> = None;
    let mut repeat: u32 = 0;

    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "-c" => {
                code = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("-c requires a code string"))?,
                );
            }
            "--repeat" => {
                repeat = args
                    .next()
                    .ok_or_else(|| anyhow!("--repeat requires N"))?
                    .parse()
                    .context("parse --repeat value")?;
            }
            other => {
                if script_path.is_some() {
                    return Err(anyhow!("unexpected argument: {other}\n{USAGE}"));
                }
                script_path = Some(PathBuf::from(other));
            }
        }
    }

    let code = match (code, script_path) {
        (Some(c), None) => c,
        (None, Some(p)) => std::fs::read_to_string(&p)
            .with_context(|| format!("read script {}", p.display()))?,
        (Some(_), Some(_)) => {
            return Err(anyhow!("pass either -c <code> OR <script.py>, not both"));
        }
        (None, None) => {
            print!("{USAGE}");
            return Err(anyhow!("no script or -c code provided"));
        }
    };

    let kernel = default_kernel();
    let initrd = default_initrd();
    require_exists(
        "kernel",
        &kernel,
        "build it with `just build` in examples/python-agent-driver, or set PYHL_KERNEL",
    )?;
    require_exists(
        "initrd",
        &initrd,
        "build it with `just rootfs` in examples/python-agent-driver, or set PYHL_INITRD",
    )?;

    let t_evolve = Instant::now();
    let mut sandbox = Sandbox::builder(&kernel)
        .initrd_file(&initrd)
        // The driver preloads numpy + pandas + friends, which wants
        // well over 512 MiB. 2 GiB has held for every workload tested.
        .heap_size(2 * 1024 * 1024 * 1024)
        .build()?;
    eprintln!(
        "[pyhl] evolve={:.1}ms",
        t_evolve.elapsed().as_secs_f64() * 1000.0
    );

    // First-ever call into the guest triggers hl_pydriver's main(), which
    // does Py_Initialize + preloads numpy / pandas / etc. and registers
    // the v2 dispatch callback. We eat that cost up-front with a no-op
    // script so the user's actual code never sees it, then snapshot the
    // warm state. Every subsequent call restores to this snapshot, so
    // each user invocation runs against a clean post-warmup Python with
    // no state leak from prior runs.
    sandbox.restore()?;
    let t_warm = Instant::now();
    let _: () = sandbox.call_named("run", "pass".to_string())?;
    eprintln!(
        "[pyhl] warmup={:.1}ms",
        t_warm.elapsed().as_secs_f64() * 1000.0
    );
    sandbox.snapshot_now()?;

    let total = repeat + 1;
    for i in 1..=total {
        let t_restore = Instant::now();
        sandbox.restore()?;
        let restore_ms = t_restore.elapsed().as_secs_f64() * 1000.0;

        let t_call = Instant::now();
        let _: () = sandbox.call_named("run", code.clone())?;
        let call_ms = t_call.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "[pyhl] run {i}/{total} restore={restore_ms:.1}ms call={call_ms:.1}ms (hermetic)"
        );
    }

    Ok(())
}
