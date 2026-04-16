//! Cross-platform stderr redirection used to capture VM console output.
//!
//! Hyperlight writes guest port-I/O to the host's stderr via `eprint!`. To
//! return that output from the FFI boundary (`hl_vm_output`) and from
//! `run_vm_capture_output`, we temporarily redirect stderr to a file or pipe
//! while the VM is running, then restore it.
//!
//! On Unix this is a straight `dup2(fd, 2)` dance. On Windows we use
//! `SetStdHandle(STD_ERROR_HANDLE, ...)` with a file handle. Both variants
//! expose the same `Capture` type.

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
    use anyhow::{anyhow, Result};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    use windows::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};

    pub struct Capture {
        original_stderr: HANDLE,
        redirect_handle: HANDLE,
    }

    impl Capture {
        pub fn redirect_to_file(path: &Path) -> Result<Self> {
            unsafe {
                let original_stderr =
                    GetStdHandle(STD_ERROR_HANDLE).map_err(|e| anyhow!("GetStdHandle: {e}"))?;

                let wide: Vec<u16> = path
                    .as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();
                let redirect_handle = CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    CREATE_ALWAYS,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
                .map_err(|e| anyhow!("CreateFileW: {e}"))?;

                SetStdHandle(STD_ERROR_HANDLE, redirect_handle)
                    .map_err(|e| anyhow!("SetStdHandle(redirect): {e}"))?;

                Ok(Self {
                    original_stderr,
                    redirect_handle,
                })
            }
        }

        pub fn restore(self) -> Result<()> {
            unsafe {
                SetStdHandle(STD_ERROR_HANDLE, self.original_stderr)
                    .map_err(|e| anyhow!("SetStdHandle(restore): {e}"))?;
                let _ = CloseHandle(self.redirect_handle);
            }
            Ok(())
        }
    }
}

pub use imp::Capture;
