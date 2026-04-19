//! pyhl — run Python on hyperlight-unikraft with a persistent warmed interpreter.
//!
//! The binary wraps two things:
//!   `pyhl setup`  — installs the python-agent-driver image (kernel + CPIO) into
//!                   .pyhl/ so `pyhl run` can find it without the user having
//!                   to juggle paths.
//!   `pyhl run`    — runs a Python file or inline snippet against the installed
//!                   image. First call of the process pays the ~3.5s Py_Initialize
//!                   + warm-import cost; every subsequent invocation uses the
//!                   post-warmup snapshot and runs in ~100ms hermetic.
//!
//! Image resolution order, first hit wins:
//!   1. --dest PATH            (on the command line)
//!   2. $PYHL_HOME             (env var)
//!   3. ./.pyhl/               (cwd-relative)
//!   4. ~/.local/share/pyhl/   (XDG fallback)
//!
//! An installed image is just two files plus a metadata stamp:
//!   <home>/kernel           Unikraft kernel ELF
//!   <home>/initrd.cpio      driver + preloaded Python deps
//!   <home>/VERSION          source + timestamp (informational)

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use hyperlight_unikraft::Sandbox;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "pyhl", version, about = "Run Python on hyperlight-unikraft")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install the python-agent-driver image (kernel + CPIO) so `pyhl run` can find it.
    Setup(SetupArgs),

    /// Run Python code against the installed image.
    Run(RunArgs),
}

#[derive(Args)]
struct SetupArgs {
    /// Where to install. Defaults to ./.pyhl/ (or ~/.local/share/pyhl/ if cwd
    /// is not writable). Also honors $PYHL_HOME.
    #[arg(long, env = "PYHL_HOME")]
    dest: Option<PathBuf>,

    /// Copy the image from a local python-agent-driver build directory instead
    /// of downloading. The directory must contain a .unikraft/build tree with
    /// a compiled kernel and a *-initrd.cpio alongside.
    ///
    /// Typical value: path to examples/python-agent-driver in a checkout of
    /// danbugs/hyperlight-unikraft.
    #[arg(long, value_name = "DIR")]
    from: Option<PathBuf>,

    /// Overwrite an existing installed image without prompting.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct RunArgs {
    /// Path to a Python script. Mutually exclusive with -c.
    script: Option<PathBuf>,

    /// Inline Python code. Mutually exclusive with <SCRIPT>.
    #[arg(short = 'c', long = "code", value_name = "CODE")]
    code: Option<String>,

    /// Run this many ADDITIONAL times after the first (each invocation is
    /// hermetic — fresh Python state via snapshot/restore).
    #[arg(long, default_value_t = 0, value_name = "N")]
    repeat: u32,

    /// Override the image directory.
    #[arg(long, env = "PYHL_HOME", value_name = "DIR")]
    dest: Option<PathBuf>,
}

// -- image-home resolution ----------------------------------------------------

const CWD_HOME: &str = ".pyhl";
const KERNEL_FILE: &str = "kernel";
const INITRD_FILE: &str = "initrd.cpio";
const VERSION_FILE: &str = "VERSION";

/// Resolve the image home to use. Tries (in order): explicit, PYHL_HOME,
/// ./.pyhl/, ~/.local/share/pyhl/. For `run`, the first one that already
/// contains a usable image is picked. For `setup`, the first writable one
/// is picked.
fn resolve_home(explicit: Option<&Path>, mode: ResolveMode) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    let cwd = std::env::current_dir()
        .context("read cwd")?
        .join(CWD_HOME);
    let xdg = xdg_share_home().join("pyhl");

    match mode {
        ResolveMode::ForRun => {
            if image_installed(&cwd) {
                return Ok(cwd);
            }
            if image_installed(&xdg) {
                return Ok(xdg);
            }
            Err(anyhow!(
                "no pyhl image installed.\n\
                 searched: {}, {}\n\
                 run `pyhl setup --from <path/to/python-agent-driver>` first.",
                cwd.display(),
                xdg.display()
            ))
        }
        ResolveMode::ForSetup => {
            // Default to cwd-local to keep the artifact close to the project;
            // caller can override with --dest/$PYHL_HOME.
            Ok(cwd)
        }
    }
}

enum ResolveMode {
    ForRun,
    ForSetup,
}

fn xdg_share_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"));
            home.join(".local/share")
        })
}

fn image_installed(home: &Path) -> bool {
    home.join(KERNEL_FILE).is_file() && home.join(INITRD_FILE).is_file()
}

// -- `setup` ------------------------------------------------------------------

fn cmd_setup(args: SetupArgs) -> Result<()> {
    let home = resolve_home(args.dest.as_deref(), ResolveMode::ForSetup)?;

    let from = args.from.as_deref().ok_or_else(|| {
        anyhow!(
            "pyhl setup currently requires --from <DIR>.\n\
             Point it at a built examples/python-agent-driver/ tree:\n  \
             pyhl setup --from /path/to/hyperlight-unikraft/examples/python-agent-driver\n\
             (Remote artifact download will be added once GitHub Releases ship the image.)"
        )
    })?;

    let (src_kernel, src_initrd) = discover_source_artifacts(from)
        .with_context(|| format!("scanning {} for image artifacts", from.display()))?;

    fs::create_dir_all(&home)
        .with_context(|| format!("create image home {}", home.display()))?;

    let dst_kernel = home.join(KERNEL_FILE);
    let dst_initrd = home.join(INITRD_FILE);
    let dst_version = home.join(VERSION_FILE);

    if image_installed(&home) && !args.force {
        eprintln!(
            "pyhl: image already installed at {} (use --force to overwrite)",
            home.display()
        );
        eprintln!("  kernel:  {}", dst_kernel.display());
        eprintln!("  initrd:  {}", dst_initrd.display());
        return Ok(());
    }

    copy_replace(&src_kernel, &dst_kernel)
        .with_context(|| format!("install {}", dst_kernel.display()))?;
    copy_replace(&src_initrd, &dst_initrd)
        .with_context(|| format!("install {}", dst_initrd.display()))?;

    let version = format!(
        "pyhl {pyhl_ver}\nsource: {src}\nkernel: {kern}\ninitrd: {initrd}\ninstalled: {ts}\n",
        pyhl_ver = env!("CARGO_PKG_VERSION"),
        src = from.display(),
        kern = src_kernel.display(),
        initrd = src_initrd.display(),
        ts = now_iso8601(),
    );
    fs::write(&dst_version, version)?;

    eprintln!("pyhl: installed image to {}", home.display());
    eprintln!("  kernel:  {} ({} MiB)", dst_kernel.display(), mib(&dst_kernel));
    eprintln!("  initrd:  {} ({} MiB)", dst_initrd.display(), mib(&dst_initrd));
    Ok(())
}

/// Copy `src` → `dst`, replacing `dst` if it exists. Uses rename-into-place
/// for the actual swap so we don't leave a half-written file on failure.
fn copy_replace(src: &Path, dst: &Path) -> Result<()> {
    let staging = dst.with_extension("pyhl.tmp");
    let _ = fs::remove_file(&staging);
    fs::copy(src, &staging)?;
    fs::rename(&staging, dst)?;
    Ok(())
}

/// Given a python-agent-driver directory, find the built kernel (under
/// `.unikraft/build/*_hyperlight-x86_64`) and the initrd CPIO (any
/// `*-initrd.cpio` in the directory root).
fn discover_source_artifacts(dir: &Path) -> Result<(PathBuf, PathBuf)> {
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let build = dir.join(".unikraft/build");
    let kernel = if build.is_dir() {
        fs::read_dir(&build)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with("_hyperlight-x86_64") && !n.ends_with(".dbg"))
                    .unwrap_or(false)
            })
    } else {
        None
    }
    .ok_or_else(|| {
        anyhow!(
            "no built kernel under {} — run `just build` (or \
             kraft-hyperlight build --plat hyperlight --arch x86_64) \
             in {} first",
            build.display(),
            dir.display()
        )
    })?;

    let initrd = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with("-initrd.cpio"))
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            anyhow!(
                "no *-initrd.cpio in {} — run `just rootfs` there first",
                dir.display()
            )
        })?;

    Ok((kernel, initrd))
}

fn mib(p: &Path) -> u64 {
    fs::metadata(p).map(|m| m.len() / 1024 / 1024).unwrap_or(0)
}

/// Lightweight timestamp (seconds since epoch in ISO-8601-ish) so we don't
/// need to pull chrono just for the VERSION stamp.
fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

// -- `run` --------------------------------------------------------------------

fn cmd_run(args: RunArgs) -> Result<()> {
    let code = match (args.script.as_deref(), args.code.as_deref()) {
        (Some(_), Some(_)) => bail!("pass either <SCRIPT> or -c <CODE>, not both"),
        (Some(p), None) => {
            fs::read_to_string(p).with_context(|| format!("read script {}", p.display()))?
        }
        (None, Some(c)) => c.to_string(),
        (None, None) => bail!("provide a script path or -c <CODE>"),
    };

    let home = resolve_home(args.dest.as_deref(), ResolveMode::ForRun)?;
    let kernel = home.join(KERNEL_FILE);
    let initrd = home.join(INITRD_FILE);

    let t_evolve = Instant::now();
    let mut sandbox = Sandbox::builder(&kernel)
        .initrd_file(&initrd)
        // 2 GiB has held for every workload tested — the driver preloads
        // numpy + pandas + friends, which alone wants ~500 MiB of heap
        // plus headroom for user code.
        .heap_size(2 * 1024 * 1024 * 1024)
        .build()?;
    eprintln!(
        "[pyhl] evolve={:.1}ms",
        t_evolve.elapsed().as_secs_f64() * 1000.0
    );

    // First call into the guest triggers hl_pydriver's main(): Py_Initialize
    // + preload 17 modules + register the v2 dispatch callback. We eat that
    // cost up-front with a no-op script so the user's actual code never sees
    // it, then snapshot_now() captures the warm state. Every subsequent
    // call restore()s to that snapshot, so each user run is hermetic —
    // __main__ globals, sys.modules accumulated during the run, etc. don't
    // leak into the next one.
    sandbox.restore()?;
    let t_warm = Instant::now();
    let _: () = sandbox.call_named("run", "pass".to_string())?;
    eprintln!(
        "[pyhl] warmup={:.1}ms",
        t_warm.elapsed().as_secs_f64() * 1000.0
    );
    sandbox.snapshot_now()?;

    let total = args.repeat + 1;
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

// -- main ---------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Command::Setup(args) => cmd_setup(args),
        Command::Run(args) => cmd_run(args),
    }
}
