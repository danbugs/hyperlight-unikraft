//! C FFI for embedding hyperlight-unikraft in non-Rust hosts (e.g., Go via CGo).
//!
//! This module exposes a thread-safe, opaque-handle-based API for creating,
//! running, and managing Hyperlight-backed Unikraft VMs from C/Go code.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::prepend_cmdline_to_initrd;
use hyperlight_host::sandbox::uninitialized::GuestEnvironment;
use hyperlight_host::sandbox::SandboxConfiguration;
use hyperlight_host::{GuestBinary, UninitializedSandbox};

/// VM status codes exposed to C.
pub const HL_STATUS_CREATED: i32 = 0;
pub const HL_STATUS_RUNNING: i32 = 1;
pub const HL_STATUS_STOPPED: i32 = 2;
pub const HL_STATUS_ERROR: i32 = 3;

/// Opaque VM handle. All fields are thread-safe.
pub struct HlVm {
    status: AtomicI32,
    output: Arc<Mutex<String>>,
    error: Mutex<Option<CString>>,
    output_cstr: Mutex<Option<CString>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    // Config stored for deferred sandbox creation (sandbox is not Send)
    kernel_path: String,
    initrd_data: Option<Vec<u8>>,
    heap_size: u64,
    stack_size: u64,
}

/// Configuration passed from C to create a VM.
#[repr(C)]
pub struct HlConfig {
    pub kernel_path: *const c_char,
    pub initrd_path: *const c_char, // nullable
    pub app_args: *const *const c_char, // nullable, null-terminated array
    pub app_args_count: c_int,
    pub heap_size: u64,
    pub stack_size: u64,
}

thread_local! {
    static LAST_ERROR: std::cell::RefCell<Option<CString>> = const { std::cell::RefCell::new(None) };
}

fn set_last_error(msg: &str) {
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = CString::new(msg).ok();
    });
}

/// Get the last error message from the current thread. Returns NULL if no error.
/// The returned pointer is valid until the next FFI call on the same thread.
#[unsafe(no_mangle)]
pub extern "C" fn hl_last_error() -> *const c_char {
    LAST_ERROR.with(|e| {
        e.borrow()
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null())
    })
}

/// Create a new VM handle from the given configuration.
///
/// The initrd file (if specified) is read and mmap'd here. The actual Hyperlight
/// sandbox is created lazily when `hl_vm_start` is called, because sandbox
/// creation must happen on the thread that runs it.
///
/// Returns NULL on failure (check `hl_last_error`).
#[unsafe(no_mangle)]
pub extern "C" fn hl_vm_create(config: *const HlConfig) -> *mut HlVm {
    let config = unsafe {
        if config.is_null() {
            set_last_error("config is null");
            return std::ptr::null_mut();
        }
        &*config
    };

    let kernel_path = unsafe {
        if config.kernel_path.is_null() {
            set_last_error("kernel_path is null");
            return std::ptr::null_mut();
        }
        match CStr::from_ptr(config.kernel_path).to_str() {
            Ok(s) => s.to_string(),
            Err(e) => {
                set_last_error(&format!("invalid kernel_path: {}", e));
                return std::ptr::null_mut();
            }
        }
    };

    // Read initrd file if specified
    let initrd_data = if !config.initrd_path.is_null() {
        let initrd_path = unsafe {
            match CStr::from_ptr(config.initrd_path).to_str() {
                Ok(s) => s.to_string(),
                Err(e) => {
                    set_last_error(&format!("invalid initrd_path: {}", e));
                    return std::ptr::null_mut();
                }
            }
        };
        match std::fs::read(&initrd_path) {
            Ok(data) => Some(data),
            Err(e) => {
                set_last_error(&format!("failed to read initrd {}: {}", initrd_path, e));
                return std::ptr::null_mut();
            }
        }
    } else {
        None
    };

    // Parse app args
    let app_args: Vec<String> = if !config.app_args.is_null() && config.app_args_count > 0 {
        (0..config.app_args_count)
            .filter_map(|i| unsafe {
                let ptr = *config.app_args.add(i as usize);
                if ptr.is_null() {
                    None
                } else {
                    CStr::from_ptr(ptr).to_str().ok().map(|s| s.to_string())
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    // Prepend cmdline to initrd if we have app args
    let initrd_data = prepend_cmdline_to_initrd(initrd_data.as_deref(), &app_args)
        .or(initrd_data);

    let vm = Box::new(HlVm {
        status: AtomicI32::new(HL_STATUS_CREATED),
        output: Arc::new(Mutex::new(String::new())),
        error: Mutex::new(None),
        output_cstr: Mutex::new(None),
        thread: Mutex::new(None),
        kernel_path,
        initrd_data,
        heap_size: config.heap_size,
        stack_size: config.stack_size,
    });

    Box::into_raw(vm)
}

/// Start the VM in a background thread.
///
/// The sandbox is created and evolved on the background thread.
/// Returns 0 on success, -1 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn hl_vm_start(vm: *mut HlVm) -> c_int {
    let vm = unsafe {
        if vm.is_null() {
            set_last_error("vm is null");
            return -1;
        }
        &*vm
    };

    let expected = HL_STATUS_CREATED;
    if vm
        .status
        .compare_exchange(expected, HL_STATUS_RUNNING, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        set_last_error("VM is not in CREATED state");
        return -1;
    }

    let kernel_path = vm.kernel_path.clone();
    let initrd_data = vm.initrd_data.clone();
    let heap_size = vm.heap_size;
    let stack_size = vm.stack_size;
    let output = vm.output.clone();
    // We need a raw pointer to update status from the thread.
    // This is safe because the thread joins before the VM is freed.
    let vm_ptr = vm as *const HlVm as usize;

    let handle = std::thread::spawn(move || {
        let result = run_vm_on_thread(&kernel_path, initrd_data.as_deref(), heap_size, stack_size, &output);

        let vm = unsafe { &*(vm_ptr as *const HlVm) };
        match result {
            Ok(()) => {
                vm.status.store(HL_STATUS_STOPPED, Ordering::SeqCst);
            }
            Err(e) => {
                if let Ok(mut err) = vm.error.lock() {
                    *err = CString::new(e.to_string()).ok();
                }
                vm.status.store(HL_STATUS_ERROR, Ordering::SeqCst);
            }
        }
    });

    if let Ok(mut t) = vm.thread.lock() {
        *t = Some(handle);
    }

    0
}

fn run_vm_on_thread(
    kernel_path: &str,
    initrd_data: Option<&[u8]>,
    heap_size: u64,
    stack_size: u64,
    output: &Arc<Mutex<String>>,
) -> anyhow::Result<()> {
    use std::io::{Read as _, Write as _};
    use std::os::unix::io::AsRawFd;

    let path = Path::new(kernel_path);
    if !path.exists() {
        return Err(anyhow::anyhow!("kernel not found: {}", kernel_path));
    }

    let mut sandbox_config = SandboxConfiguration::default();
    // In v0.13.1+, stack is part of the guest heap memory region
    sandbox_config.set_heap_size(heap_size + stack_size);

    let env = GuestEnvironment::new(
        GuestBinary::FilePath(kernel_path.to_string()),
        initrd_data,
    );

    let sandbox = UninitializedSandbox::new(env, Some(sandbox_config))?;

    // Capture stderr: Unikraft console outputs via port I/O which Hyperlight
    // writes to stderr using eprint!. We redirect stderr to a pipe on this
    // thread to capture it.
    let (read_fd, write_fd) = nix::unistd::pipe()?;
    let original_stderr = nix::unistd::dup(2)?;
    nix::unistd::dup2(write_fd.as_raw_fd(), 2)?;

    // Evolve runs the unikernel to completion (blocks until HLT)
    match sandbox.evolve() {
        Ok(_) | Err(_) => {} // HLT is expected for unikernels
    }

    // Restore stderr and read captured output
    std::io::stderr().flush().ok();
    nix::unistd::dup2(original_stderr.as_raw_fd(), 2)?;
    let _ = original_stderr;
    let _ = write_fd;

    let mut captured = String::new();
    let mut reader = std::fs::File::from(read_fd);
    let flags = nix::fcntl::fcntl(reader.as_raw_fd(), nix::fcntl::FcntlArg::F_GETFL)?;
    nix::fcntl::fcntl(
        reader.as_raw_fd(),
        nix::fcntl::FcntlArg::F_SETFL(
            nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK,
        ),
    )?;
    reader.read_to_string(&mut captured).ok();

    if let Ok(mut buf) = output.lock() {
        *buf = captured;
    }

    Ok(())
}

/// Get the current VM status.
///
/// Returns: 0=CREATED, 1=RUNNING, 2=STOPPED, 3=ERROR
#[unsafe(no_mangle)]
pub extern "C" fn hl_vm_status(vm: *const HlVm) -> c_int {
    let vm = unsafe {
        if vm.is_null() {
            return -1;
        }
        &*vm
    };
    vm.status.load(Ordering::SeqCst)
}

/// Wait for the VM to finish. Blocks until the background thread exits.
/// Returns 0 on success, -1 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn hl_vm_wait(vm: *mut HlVm) -> c_int {
    let vm = unsafe {
        if vm.is_null() {
            set_last_error("vm is null");
            return -1;
        }
        &*vm
    };

    let handle = {
        let mut t = match vm.thread.lock() {
            Ok(t) => t,
            Err(_) => {
                set_last_error("thread mutex poisoned");
                return -1;
            }
        };
        t.take()
    };

    if let Some(handle) = handle {
        if handle.join().is_err() {
            set_last_error("thread panicked");
            return -1;
        }
    }

    0
}

/// Get captured output from the VM. Valid after VM stops.
///
/// Returns a pointer to a null-terminated UTF-8 string. The pointer is valid
/// until the next call to `hl_vm_output` or `hl_vm_free` on the same VM.
/// Returns NULL if vm is null.
#[unsafe(no_mangle)]
pub extern "C" fn hl_vm_output(vm: *const HlVm) -> *const c_char {
    let vm = unsafe {
        if vm.is_null() {
            return std::ptr::null();
        }
        &*vm
    };

    let output = match vm.output.lock() {
        Ok(o) => o.clone(),
        Err(_) => return std::ptr::null(),
    };

    let cstr = match CString::new(output) {
        Ok(s) => s,
        Err(_) => return std::ptr::null(),
    };

    let ptr = cstr.as_ptr();
    if let Ok(mut cached) = vm.output_cstr.lock() {
        *cached = Some(cstr);
    }
    ptr
}

/// Get the error message if VM status is ERROR.
/// Returns NULL if no error or vm is null.
#[unsafe(no_mangle)]
pub extern "C" fn hl_vm_error(vm: *const HlVm) -> *const c_char {
    let vm = unsafe {
        if vm.is_null() {
            return std::ptr::null();
        }
        &*vm
    };

    match vm.error.lock() {
        Ok(e) => e.as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null()),
        Err(_) => std::ptr::null(),
    }
}

/// Free the VM handle. Waits for any running thread to complete first.
#[unsafe(no_mangle)]
pub extern "C" fn hl_vm_free(vm: *mut HlVm) {
    if vm.is_null() {
        return;
    }

    // Wait for thread to finish before freeing
    hl_vm_wait(vm);

    unsafe {
        drop(Box::from_raw(vm));
    }
}
