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
use hyperlight_unikraft::pyhl::{copy_replace, discover_source_artifacts};
use hyperlight_unikraft::{Preopen, Sandbox};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Parse a `--mount HOST[:GUEST]` argument to a `Preopen`. Default guest
/// path is `/host` when omitted.
fn parse_mount(spec: &str) -> Result<Preopen> {
    Preopen::parse_cli(spec).map_err(|e| anyhow!("invalid --mount {:?}: {}", spec, e))
}

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

    /// Install from a local python-agent-driver build directory instead of
    /// downloading from GHCR. The directory must contain a `.unikraft/build`
    /// tree with a compiled kernel and a `*-initrd.cpio` alongside —
    /// typically `examples/python-agent-driver` in a checkout of
    /// danbugs/hyperlight-unikraft after `just build && just rootfs`.
    ///
    /// Without --from, pyhl pulls the pre-published image from GHCR (requires
    /// docker or podman on $PATH).
    #[arg(long, value_name = "DIR")]
    from: Option<PathBuf>,

    /// Overwrite an existing installed image without prompting.
    #[arg(long)]
    force: bool,

    /// Expose a host directory to the guest at a fixed guest path.
    /// Format: HOST_DIR[:GUEST_PATH] (default GUEST_PATH is `/host`).
    /// Repeat for multiple mounts.
    ///
    /// The *guest path* is baked into the persisted snapshot (the guest
    /// mounts hostfs during warmup), so `pyhl run --mount` can only
    /// remap the host side later — the guest path must match what was
    /// given to `setup`.
    #[arg(long = "mount", value_name = "HOST[:GUEST]")]
    mounts: Vec<String>,
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

    /// Expose a host directory to the guest for this run. Same format
    /// as `pyhl setup --mount`. The guest-path must match what was
    /// baked into the snapshot at setup time; only the host side is
    /// remappable per-run.
    #[arg(long = "mount", value_name = "HOST[:GUEST]")]
    mounts: Vec<String>,

    /// Print evolve/warmup/per-run timing to stderr. Off by default so the
    /// user's script output is clean.
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,
}

// -- image-home resolution ----------------------------------------------------

const CWD_HOME: &str = ".pyhl";
const KERNEL_FILE: &str = "kernel";
const INITRD_FILE: &str = "initrd.cpio";
const SNAPSHOT_FILE: &str = "snapshot.hls";
const VERSION_FILE: &str = "VERSION";

/// Resolve the image home to use. Tries (in order): explicit, PYHL_HOME,
/// ./.pyhl/, ~/.local/share/pyhl/. For `run`, the first one that already
/// contains a usable image is picked. For `setup`, the first writable one
/// is picked.
fn resolve_home(explicit: Option<&Path>, mode: ResolveMode) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    let cwd = std::env::current_dir().context("read cwd")?.join(CWD_HOME);
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

/// GHCR OCI image references published by .github/workflows/publish-examples.yml.
/// Both images are FROM-scratch payloads: kernel image has a single /kernel,
/// initrd image has a single /initrd.cpio.
const GHCR_KERNEL_IMAGE: &str =
    "ghcr.io/danbugs/hyperlight-unikraft/python-agent-driver-kernel:latest";
const GHCR_INITRD_IMAGE: &str =
    "ghcr.io/danbugs/hyperlight-unikraft/python-agent-driver-initrd:latest";

fn cmd_setup(args: SetupArgs) -> Result<()> {
    let home = resolve_home(args.dest.as_deref(), ResolveMode::ForSetup)?;

    let dst_kernel = home.join(KERNEL_FILE);
    let dst_initrd = home.join(INITRD_FILE);
    let dst_snapshot = home.join(SNAPSHOT_FILE);
    let dst_version = home.join(VERSION_FILE);

    if image_installed(&home) && dst_snapshot.is_file() && !args.force {
        eprintln!(
            "pyhl: image already installed at {} (use --force to overwrite)",
            home.display()
        );
        eprintln!("  kernel:   {}", dst_kernel.display());
        eprintln!("  initrd:   {}", dst_initrd.display());
        eprintln!("  snapshot: {}", dst_snapshot.display());
        return Ok(());
    }

    fs::create_dir_all(&home).with_context(|| format!("create image home {}", home.display()))?;

    let (source_label, src_kernel, src_initrd) = match args.from.as_deref() {
        Some(dir) => {
            let (k, i) = discover_source_artifacts(dir)
                .with_context(|| format!("scanning {} for image artifacts", dir.display()))?;
            (dir.display().to_string(), k, i)
        }
        None => {
            // No --from: pull from GHCR. Uses docker or podman under the hood
            // because that's the standard OCI client everyone has and avoids
            // linking an oci-distribution client into pyhl.
            eprintln!("pyhl: downloading image from GHCR…");
            let tmp = home.join(".pyhl.download");
            fs::create_dir_all(&tmp)?;
            let kernel_path = tmp.join("kernel");
            let initrd_path = tmp.join("initrd.cpio");
            extract_from_ghcr(GHCR_KERNEL_IMAGE, "/kernel", &kernel_path)?;
            extract_from_ghcr(GHCR_INITRD_IMAGE, "/initrd.cpio", &initrd_path)?;
            (
                format!("{GHCR_KERNEL_IMAGE} + {GHCR_INITRD_IMAGE}"),
                kernel_path,
                initrd_path,
            )
        }
    };

    copy_replace(&src_kernel, &dst_kernel)
        .with_context(|| format!("install {}", dst_kernel.display()))?;
    copy_replace(&src_initrd, &dst_initrd)
        .with_context(|| format!("install {}", dst_initrd.display()))?;

    // Remove the download scratch dir if we made one.
    let scratch = home.join(".pyhl.download");
    if scratch.is_dir() {
        let _ = fs::remove_dir_all(&scratch);
    }

    // Warm up a sandbox, take a snapshot after Py_Initialize + preloaded
    // imports, and persist it to disk. Every subsequent `pyhl run`
    // will MultiUseSandbox::from_snapshot() this file, which skips both
    // kernel boot (evolve) and the 3.5s Python warmup — the whole cost
    // is paid here, once.
    //
    // If --mount was passed, also tell the guest to mount hostfs at the
    // given guest path(s) during boot. The guest-side mount point is
    // baked into the snapshot's memory image; at `pyhl run --mount` the
    // host_dir side is remappable but the guest path is fixed.
    let setup_preopens: Vec<Preopen> = args
        .mounts
        .iter()
        .map(|m| parse_mount(m))
        .collect::<Result<_>>()?;

    eprintln!("pyhl: warming up Python and persisting snapshot…");
    let t_warm = Instant::now();
    {
        let mut builder = Sandbox::builder(&dst_kernel)
            .initrd_file(&dst_initrd)
            .heap_size(2 * 1024 * 1024 * 1024);
        for p in &setup_preopens {
            builder = builder.preopen(p.clone());
        }
        let mut sbox = builder.build()?;
        sbox.restore()?;
        let _: () = sbox.call_named("run", "pass".to_string())?;
        sbox.snapshot_now()?;
        sbox.save_snapshot(&dst_snapshot)?;
    }
    eprintln!(
        "pyhl:   warmup + persist = {:.1}s (one-time)",
        t_warm.elapsed().as_secs_f64()
    );

    let version = format!(
        "pyhl {pyhl_ver}\nsource: {src}\nkernel: {kern}\ninitrd: {initrd}\nsnapshot: {snap}\ninstalled: {ts}\n",
        pyhl_ver = env!("CARGO_PKG_VERSION"),
        src = source_label,
        kern = src_kernel.display(),
        initrd = src_initrd.display(),
        snap = dst_snapshot.display(),
        ts = now_iso8601(),
    );
    fs::write(&dst_version, version)?;

    eprintln!("pyhl: installed image to {}", home.display());
    eprintln!(
        "  kernel:   {} ({} MiB)",
        dst_kernel.display(),
        mib(&dst_kernel)
    );
    eprintln!(
        "  initrd:   {} ({} MiB)",
        dst_initrd.display(),
        mib(&dst_initrd)
    );
    eprintln!(
        "  snapshot: {} ({} MiB)",
        dst_snapshot.display(),
        mib(&dst_snapshot)
    );
    Ok(())
}

/// Pull an OCI image from GHCR and extract a single file out of it. The
/// workflow-published kernel/initrd images are FROM-scratch with a single
/// payload file at a known path (/kernel or /initrd.cpio), so this is a
/// straightforward `pull → create → cp → rm` dance.
///
/// Uses whichever of `docker` / `podman` is on PATH. Both speak the same
/// OCI protocol to ghcr.io and need no auth for public pulls.
fn extract_from_ghcr(image: &str, src_path_in_image: &str, dst: &Path) -> Result<()> {
    use std::process::Command;

    let tool = find_on_path(&["docker", "podman"]).ok_or_else(|| {
        anyhow!(
            "need `docker` or `podman` on $PATH to pull the pyhl image from GHCR.\n\
             alternatives: install one of them, or use `pyhl setup --from <local-dir>`"
        )
    })?;

    let run = |cmd: &mut Command, label: &str| -> Result<std::process::Output> {
        let out = cmd
            .output()
            .with_context(|| format!("spawn {tool} {label}"))?;
        if !out.status.success() {
            bail!(
                "{tool} {label} failed (exit {:?}): {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(out)
    };

    eprintln!("pyhl:   pull {image}");
    run(Command::new(tool).args(["pull", image]), "pull")?;

    // Pick a container name unlikely to collide with user containers.
    let cname = format!("pyhl-extract-{}", std::process::id());
    let _ = Command::new(tool).args(["rm", "-f", &cname]).output();

    let create_out = run(
        Command::new(tool).args(["create", "--name", &cname, image, "/bogus-entry"]),
        "create",
    )?;
    let _ = String::from_utf8_lossy(&create_out.stdout);

    let cleanup = scopeguard_cleanup(tool, &cname);

    run(
        Command::new(tool).args([
            "cp",
            &format!("{cname}:{src_path_in_image}"),
            dst.to_str().expect("dst is utf-8"),
        ]),
        "cp",
    )?;

    drop(cleanup);
    Ok(())
}

/// Minimal "run this on drop" helper so the tmp container always gets
/// removed, even if `cp` fails partway through.
fn scopeguard_cleanup(tool: &str, cname: &str) -> CleanupContainer {
    CleanupContainer {
        tool: tool.to_string(),
        cname: cname.to_string(),
    }
}

struct CleanupContainer {
    tool: String,
    cname: String,
}
impl Drop for CleanupContainer {
    fn drop(&mut self) {
        let _ = std::process::Command::new(&self.tool)
            .args(["rm", "-f", &self.cname])
            .output();
    }
}

/// Return the first name in `names` that is present as an executable on the
/// user's PATH. Avoids a `which` crate dep — we only need a boolean check.
fn find_on_path(names: &[&'static str]) -> Option<&'static str> {
    let path = std::env::var_os("PATH")?;
    for name in names {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if let Ok(md) = candidate.metadata() {
                if md.is_file() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if md.permissions().mode() & 0o111 != 0 {
                            return Some(name);
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        return Some(name);
                    }
                }
            }
        }
    }
    None
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
    let snapshot = home.join(SNAPSHOT_FILE);

    // Fast path: `pyhl setup` already warmed up a sandbox, ran
    // Py_Initialize + preloaded modules, captured the state, and
    // persisted it to snapshot.hls. Here we mmap that file back and
    // instantiate a sandbox directly — no kernel boot, no Python init.
    if !snapshot.is_file() {
        return Err(anyhow!(
            "no warmed-up snapshot at {}.\n\
             run `pyhl setup` first (or `pyhl setup --force` if you have\n\
             an older install without the snapshot file).",
            snapshot.display()
        ));
    }

    // If --mount was passed, parse the specs and register fs_* host
    // handlers on the loaded sandbox so guest file I/O routes back here.
    let run_preopens: Vec<Preopen> = args
        .mounts
        .iter()
        .map(|m| parse_mount(m))
        .collect::<Result<_>>()?;

    let t_load = Instant::now();
    let mut sandbox = if run_preopens.is_empty() {
        Sandbox::from_snapshot_file(&snapshot)?
    } else {
        Sandbox::from_snapshot_file_with(&snapshot, &run_preopens)?
    };
    if args.verbose {
        eprintln!(
            "[pyhl] load_snapshot={:.1}ms",
            t_load.elapsed().as_secs_f64() * 1000.0
        );
    }

    // The loaded snapshot IS the warm state. On the first iteration we
    // can go straight to `call` — the sandbox is already at that state.
    // Restore between subsequent iterations to keep them hermetic
    // (rewinds globals + any stdout buffering from the previous call).
    let total = args.repeat + 1;
    for i in 1..=total {
        let restore_ms = if i == 1 {
            0.0
        } else {
            let t_restore = Instant::now();
            sandbox.restore()?;
            t_restore.elapsed().as_secs_f64() * 1000.0
        };

        let t_call = Instant::now();
        let _: () = sandbox.call_named("run", code.clone())?;
        let call_ms = t_call.elapsed().as_secs_f64() * 1000.0;
        if args.verbose {
            eprintln!(
                "[pyhl] run {i}/{total} restore={restore_ms:.1}ms call={call_ms:.1}ms (hermetic)"
            );
        }
    }

    Ok(())
}

// -- main ---------------------------------------------------------------------

fn main() -> Result<()> {
    // On Windows, hyperlight's surrogate-process manager pre-spawns
    // HYPERLIGHT_INITIAL_SURROGATES Windows processes (default 512)
    // the first time any sandbox is created. At ~7ms per CreateProcessA
    // that's ~3.5s of amortised cost we pay on every `pyhl run`. Since
    // pyhl is a short-lived single-sandbox CLI, pinning the initial
    // count at 1 drops that to ~7ms. Caller can override by setting
    // the env var explicitly before invoking pyhl.
    if std::env::var_os("HYPERLIGHT_INITIAL_SURROGATES").is_none() {
        // Safety: main() runs single-threaded on entry; set_var is safe here.
        unsafe {
            std::env::set_var("HYPERLIGHT_INITIAL_SURROGATES", "1");
        }
    }

    let cli = Cli::parse();
    match cli.cmd {
        Command::Setup(args) => cmd_setup(args),
        Command::Run(args) => cmd_run(args),
    }
}
