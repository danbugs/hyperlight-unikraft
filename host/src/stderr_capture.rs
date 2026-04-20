//! Cross-platform stderr redirection used to capture VM console output.
//!
//! On Unix: dup2-based redirect to a temp file.
//! On Windows: no-op (VM output goes to inherited stderr, which the
//! kraftkit subprocess driver captures via exec.Command).

#[cfg(unix)]
mod imp {
    use anyhow::Result;
    use nix::unistd;
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
    use std::path::Path;

    pub struct Capture {
        original_stderr: OwnedFd,
    }

    impl Capture {
        pub fn redirect_to_file(path: &Path) -> Result<Self> {
            let capture_fd = std::fs::File::create(path)?.into_raw_fd();
            let original_stderr_raw = unistd::dup(2)?;
            unistd::dup2(capture_fd, 2)?;
            unistd::close(capture_fd)?;
            let original_stderr = unsafe { OwnedFd::from_raw_fd(original_stderr_raw) };
            Ok(Self { original_stderr })
        }

        pub fn restore(self) -> Result<()> {
            unistd::dup2(self.original_stderr.as_raw_fd(), 2)?;
            Ok(())
        }
    }
}

#[cfg(windows)]
mod imp {
    use anyhow::Result;
    use std::path::Path;

    /// No-op on Windows. VM console output goes to inherited stderr,
    /// which the kraftkit subprocess driver captures.
    pub struct Capture;

    impl Capture {
        pub fn redirect_to_file(_path: &Path) -> Result<Self> {
            Ok(Self)
        }

        pub fn restore(self) -> Result<()> {
            Ok(())
        }
    }
}

pub use imp::Capture;
