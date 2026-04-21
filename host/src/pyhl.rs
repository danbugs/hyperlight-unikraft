//! `pyhl` as a library — drive the python-agent-driver image from Rust
//! without going through the CLI.
//!
//! Two pieces:
//!
//! - [`install`] — one-time: take a source image (or a GHCR pull) and
//!   materialize `kernel`, `initrd.cpio`, and a warmed-up `snapshot.hls`
//!   in the image home.
//!
//! - [`Runtime`] — the steady-state object: holds an open
//!   [`crate::Sandbox`] loaded from the persisted snapshot, and
//!   exposes [`run_code`](Runtime::run_code) /
//!   [`run_script`](Runtime::run_script) for every subsequent invocation.
//!   Mounts can be supplied per runtime (one `Runtime`, many `run_*`
//!   calls against it — each hermetic via restore).
//!
//! Typical use:
//!
//! ```no_run
//! use hyperlight_unikraft::{pyhl, Preopen};
//! use std::path::Path;
//!
//! # fn main() -> anyhow::Result<()> {
//! let home = Path::new(".pyhl");
//! // One-time install (no-op if already present).
//! pyhl::install(&pyhl::InstallOptions {
//!     home,
//!     source: pyhl::InstallSource::Ghcr,
//!     mounts: &[],
//!     force: false,
//! })?;
//!
//! let mut rt = pyhl::Runtime::new(home, &[Preopen::new("./share", "/host")?])?;
//! rt.run_code("print('hello from rust')")?;
//! rt.run_code("print('hermetic second call')")?;  // fresh __main__ each time
//! # Ok(())
//! # }
//! ```
//!
//! The binary in `src/bin/pyhl.rs` is a thin wrapper over this API.

use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::{Preopen, Sandbox};

/// Standard file names inside an image home.
pub const KERNEL_FILE: &str = "kernel";
/// Standard file names inside an image home.
pub const INITRD_FILE: &str = "initrd.cpio";
/// Standard file names inside an image home.
pub const SNAPSHOT_FILE: &str = "snapshot.hls";
/// Standard file names inside an image home.
pub const VERSION_FILE: &str = "VERSION";

/// Configuration for [`install`].
pub struct InstallOptions<'a> {
    /// Target directory. Files are written at
    /// `{home}/{kernel,initrd.cpio,snapshot.hls,VERSION}`.
    pub home: &'a Path,

    /// Where to get the image from.
    pub source: InstallSource<'a>,

    /// Host → guest directory preopens. These are baked into the
    /// persisted snapshot (the guest mounts hostfs at `guest_path`
    /// during warmup). `Runtime::new` only remaps the host side.
    pub mounts: &'a [Preopen],

    /// Overwrite an existing install.
    pub force: bool,
}

/// Where `install` pulls its kernel and CPIO from.
#[derive(Debug)]
pub enum InstallSource<'a> {
    /// Pull the default published image from GHCR via docker/podman.
    Ghcr,
    /// Copy from a local python-agent-driver build tree.
    LocalDir(&'a Path),
    /// Explicit files — useful for custom image pipelines.
    Explicit { kernel: &'a Path, initrd: &'a Path },
}

/// Summary of an [`install`] run. Absolute paths to the installed files.
#[derive(Debug)]
pub struct InstallReport {
    pub home: PathBuf,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub snapshot: PathBuf,
    /// True if the install was a no-op (image already present and `force == false`).
    pub already_installed: bool,
    /// Wall-time of the warmup + snapshot persist step (0 when already installed).
    pub warmup_ms: f64,
}

/// Install — copy kernel + CPIO into `home`, warm up a sandbox, and
/// persist a post-warmup snapshot. Idempotent when `force == false`.
///
/// Only the `InstallSource::Ghcr` path touches the network; the others
/// are local file copies. See [`InstallSource`] for each variant's
/// semantics.
pub fn install(opts: &InstallOptions<'_>) -> Result<InstallReport> {
    let home = opts.home.to_path_buf();
    let dst_kernel = home.join(KERNEL_FILE);
    let dst_initrd = home.join(INITRD_FILE);
    let dst_snapshot = home.join(SNAPSHOT_FILE);
    let dst_version = home.join(VERSION_FILE);

    let already = dst_kernel.is_file() && dst_initrd.is_file() && dst_snapshot.is_file();
    if already && !opts.force {
        return Ok(InstallReport {
            home,
            kernel: dst_kernel,
            initrd: dst_initrd,
            snapshot: dst_snapshot,
            already_installed: true,
            warmup_ms: 0.0,
        });
    }

    fs::create_dir_all(&home).with_context(|| format!("create image home {}", home.display()))?;

    let (src_label, src_kernel, src_initrd) = match &opts.source {
        InstallSource::LocalDir(dir) => {
            let (k, i) = discover_source_artifacts(dir)
                .with_context(|| format!("scan {} for image artifacts", dir.display()))?;
            (format!("local:{}", dir.display()), k, i)
        }
        InstallSource::Explicit { kernel, initrd } => (
            "explicit".to_string(),
            kernel.to_path_buf(),
            initrd.to_path_buf(),
        ),
        InstallSource::Ghcr => {
            let scratch = home.join(".pyhl.download");
            fs::create_dir_all(&scratch)?;
            let k = scratch.join("kernel");
            let i = scratch.join("initrd.cpio");
            extract_from_ghcr(GHCR_KERNEL_IMAGE, "/kernel", &k)?;
            extract_from_ghcr(GHCR_INITRD_IMAGE, "/initrd.cpio", &i)?;
            (
                format!("ghcr: {GHCR_KERNEL_IMAGE} + {GHCR_INITRD_IMAGE}"),
                k,
                i,
            )
        }
    };

    copy_replace(&src_kernel, &dst_kernel)
        .with_context(|| format!("install {}", dst_kernel.display()))?;
    copy_replace(&src_initrd, &dst_initrd)
        .with_context(|| format!("install {}", dst_initrd.display()))?;

    // Warmup + persist.
    let t = Instant::now();
    {
        let mut builder = Sandbox::builder(&dst_kernel)
            .initrd_file(&dst_initrd)
            .heap_size(3 * 512 * 1024 * 1024);
        for p in opts.mounts {
            builder = builder.preopen(p.clone());
        }
        let mut sbox = builder.build()?;
        sbox.restore()?;
        let _: () = sbox.call_named("run", "pass".to_string())?;
        sbox.snapshot_now()?;
        sbox.save_snapshot(&dst_snapshot)?;
    }
    let warmup_ms = t.elapsed().as_secs_f64() * 1000.0;

    let version = format!(
        "pyhl {pyhl_ver}\nsource: {src}\nkernel: {kern}\ninitrd: {initrd}\nsnapshot: {snap}\n",
        pyhl_ver = env!("CARGO_PKG_VERSION"),
        src = src_label,
        kern = src_kernel.display(),
        initrd = src_initrd.display(),
        snap = dst_snapshot.display(),
    );
    fs::write(&dst_version, version)?;

    Ok(InstallReport {
        home,
        kernel: dst_kernel,
        initrd: dst_initrd,
        snapshot: dst_snapshot,
        already_installed: false,
        warmup_ms,
    })
}

/// Per-call timing for `Runtime::run_*` if you care.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunTiming {
    pub restore_ms: f64,
    pub call_ms: f64,
}

/// A pyhl runtime backed by a warmed-up snapshot. Cheap to keep around;
/// call `run_*` many times against the same instance to amortise the
/// load cost over many invocations.
pub struct Runtime {
    sandbox: Sandbox,
    /// True until the first run, when restore is still a no-op (the
    /// sandbox is already at the loaded-snapshot state).
    first_run: bool,
}

impl Runtime {
    /// Open a runtime against an existing install. Looks for
    /// `{home}/snapshot.hls` and mmap-loads it. `mounts` specify host
    /// directories to expose under the guest paths that were baked in
    /// at `install` time.
    pub fn new(home: &Path, mounts: &[Preopen]) -> Result<Self> {
        let snap = home.join(SNAPSHOT_FILE);
        if !snap.is_file() {
            bail!(
                "no snapshot at {} — run pyhl::install first",
                snap.display()
            );
        }
        let sandbox = if mounts.is_empty() {
            Sandbox::from_snapshot_file(&snap)?
        } else {
            Sandbox::from_snapshot_file_with(&snap, mounts)?
        };
        Ok(Self {
            sandbox,
            first_run: true,
        })
    }

    /// Execute a string of Python code. The call is hermetic: the
    /// guest's Python `__main__` dict is reset between calls by
    /// restoring the snapshot state.
    pub fn run_code(&mut self, code: &str) -> Result<RunTiming> {
        let mut t = RunTiming::default();
        if !self.first_run {
            let tr = Instant::now();
            self.sandbox.restore()?;
            t.restore_ms = tr.elapsed().as_secs_f64() * 1000.0;
        }
        self.first_run = false;

        let tc = Instant::now();
        let _: () = self.sandbox.call_named("run", code.to_string())?;
        t.call_ms = tc.elapsed().as_secs_f64() * 1000.0;
        Ok(t)
    }

    /// Convenience: read a file and run its contents.
    pub fn run_script(&mut self, path: &Path) -> Result<RunTiming> {
        let code =
            fs::read_to_string(path).with_context(|| format!("read script {}", path.display()))?;
        self.run_code(&code)
    }

    /// Force a restore before the next call (useful if the previous
    /// call was skipped or the caller wants a deterministic rewind
    /// point).
    pub fn reset(&mut self) -> Result<()> {
        self.sandbox.restore()?;
        self.first_run = false;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers shared with the `pyhl` binary. Marked `pub` (inside the `pyhl`
// module) so the bin crate can reuse them instead of keeping a parallel
// copy in sync.
// ---------------------------------------------------------------------------

/// Atomically copy `src` → `dst`: stage to `dst.pyhl.tmp`, then rename
/// into place so a failure doesn't leave a half-written file.
pub fn copy_replace(src: &Path, dst: &Path) -> Result<()> {
    let staging = dst.with_extension("pyhl.tmp");
    let _ = fs::remove_file(&staging);
    fs::copy(src, &staging)?;
    fs::rename(&staging, dst)?;
    Ok(())
}

/// Locate a `(kernel, initrd.cpio)` pair inside a python-agent-driver
/// build tree: `{dir}/.unikraft/build/*_hyperlight-x86_64` for the kernel
/// and `{dir}/*-initrd.cpio` for the rootfs. Used by `install` and by
/// the `pyhl setup --from` CLI path.
pub fn discover_source_artifacts(dir: &Path) -> Result<(PathBuf, PathBuf)> {
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

// ---------------------------------------------------------------------------
// GHCR image pull — shared by `pyhl setup` and any library caller going
// through InstallSource::Ghcr.
// ---------------------------------------------------------------------------

/// OCI image references published by `.github/workflows/publish-examples.yml`.
/// Both images are FROM-scratch payloads: kernel image has a single /kernel,
/// initrd image has a single /initrd.cpio.
pub const GHCR_KERNEL_IMAGE: &str =
    "ghcr.io/danbugs/hyperlight-unikraft/python-agent-driver-kernel:latest";
pub const GHCR_INITRD_IMAGE: &str =
    "ghcr.io/danbugs/hyperlight-unikraft/python-agent-driver-initrd:latest";

/// Pull a single file out of an OCI image hosted on GHCR. Uses whichever
/// of `docker` / `podman` is on `$PATH`. The published images are
/// FROM-scratch with a single payload file at a known path (`/kernel`
/// or `/initrd.cpio`), so this is a straightforward
/// `pull → create → cp → rm` dance.
///
/// Returns `Err` if neither container tool is available — callers that
/// want a pure-library alternative should use
/// [`InstallSource::LocalDir`] or [`InstallSource::Explicit`] instead.
pub fn extract_from_ghcr(image: &str, src_path_in_image: &str, dst: &Path) -> Result<()> {
    use std::process::Command;

    let tool = find_on_path(&["docker", "podman"]).ok_or_else(|| {
        anyhow!(
            "need `docker` or `podman` on $PATH to pull the pyhl image from GHCR.\n\
             alternatives: install one of them, or use `pyhl setup --from <local-dir>` \
             (library callers: InstallSource::LocalDir / ::Explicit)."
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

    let cname = format!("pyhl-extract-{}", std::process::id());
    let _ = Command::new(tool).args(["rm", "-f", &cname]).output();

    let _ = run(
        Command::new(tool).args(["create", "--name", &cname, image, "/bogus-entry"]),
        "create",
    )?;

    // Guard so the tmp container is removed even if `cp` fails.
    let _cleanup = GhcrCleanup {
        tool: tool.to_string(),
        cname: cname.clone(),
    };

    run(
        Command::new(tool).args([
            "cp",
            &format!("{cname}:{src_path_in_image}"),
            dst.to_str()
                .ok_or_else(|| anyhow!("dst is not valid UTF-8"))?,
        ]),
        "cp",
    )?;

    Ok(())
}

struct GhcrCleanup {
    tool: String,
    cname: String,
}

impl Drop for GhcrCleanup {
    fn drop(&mut self) {
        let _ = std::process::Command::new(&self.tool)
            .args(["rm", "-f", &self.cname])
            .output();
    }
}

/// Return the first name in `names` present as an executable on `$PATH`.
/// Avoids a `which` crate dep — we only need a boolean existence check.
pub fn find_on_path(names: &[&'static str]) -> Option<&'static str> {
    let path = std::env::var_os("PATH")?;
    for name in names {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if let Ok(md) = candidate.metadata() {
                if md.is_file() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if md.permissions().mode() & 0o111 == 0 {
                            continue;
                        }
                    }
                    return Some(name);
                }
            }
        }
    }
    None
}
