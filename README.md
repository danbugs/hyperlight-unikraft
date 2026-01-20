# hyperlight-unikraft

Run [Unikraft](https://unikraft.org/) unikernels on [Hyperlight](https://github.com/hyperlight-dev/hyperlight), a lightweight Virtual Machine Manager (VMM) designed for embedded use within applications.

## Overview

This project enables running Linux applications (Python, Node.js, Go, Rust, C/C++) on Hyperlight micro-VMs using Unikraft as the guest kernel. It provides:

1. **hyperlight-unikraft** - A CLI host that loads and runs Unikraft kernels on Hyperlight
2. **Example configurations** - Ready-to-use kraft configs for building various applications

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  Your Application (Python, Node.js, Go, Rust, C/C++)         │
│  (runs as ELF binary inside the VM)                          │
├──────────────────────────────────────────────────────────────┤
│  Unikraft Kernel (ELF loader + VFS + POSIX)                  │
│  - Mounts initrd as ramfs                                    │
│  - Loads and executes application ELF                        │
├──────────────────────────────────────────────────────────────┤
│  hyperlight-unikraft (embedded Hyperlight host)              │
│  - Loads kernel ELF + initrd                                 │
│  - Passes arguments via magic header in initrd               │
├──────────────────────────────────────────────────────────────┤
│  Hyperlight VMM (hypervisor interface)                       │
│  - Creates micro-VM with identity-mapped page tables         │
│  - Provides PEB structure with memory regions                │
├──────────────────────────────────────────────────────────────┤
│  KVM (Linux) / MSHV (Windows)                                │
└──────────────────────────────────────────────────────────────┘
```

### How It Works

1. **Host loads kernel and initrd**: `hyperlight-unikraft` reads the Unikraft kernel ELF and optional initrd (CPIO archive)
2. **Arguments embedded in initrd**: Application arguments are prepended to the initrd with a magic header (`HLCMDLN\0`)
3. **VM starts**: Hyperlight creates a micro-VM with identity-mapped memory and jumps to the kernel entry point
4. **Kernel extracts initrd**: Unikraft mounts the initrd as a RAM filesystem, extracts the embedded cmdline
5. **Application runs**: The ELF loader loads and executes the application binary (e.g., `/usr/bin/python3`)
6. **Output via console**: Application output goes through `outb` to port 0xE9, which Hyperlight captures

### Key Features

- **No host function calls** - The Unikraft kernel runs entirely within the VM
- **Identity-mapped memory** - Simplified memory layout (vaddr == paddr)
- **Generic cmdline mechanism** - Pass arguments to any application via `-- arg1 arg2 ...`
- **Fast cold start** - Hyperlight's lightweight design enables millisecond startup times

## Prerequisites

- **Linux with KVM support** (`/dev/kvm` with read/write access)
  - Note: Currently only KVM is supported (not MSHV) due to the `hw-interrupts` feature requirement
- Go 1.25+ (for building kraft-hyperlight)
- Docker (for building application rootfs)
- Rust 1.89+ toolchain (for building the host)
- `cpio` utility (`sudo apt install cpio`)
- `musl-gcc` for C examples (`sudo apt install musl-tools`)

## Setup

### 1. Build Kraft with Hyperlight Support

The examples use `kraft-hyperlight` to build Unikraft kernels:

```bash
git clone https://github.com/danbugs/kraftkit.git
cd kraftkit
git checkout hyperlight-platform
go build -o kraft-hyperlight ./cmd/kraft
sudo mv kraft-hyperlight /usr/local/bin/
```

### 2. Build the Host

```bash
cd host
cargo build --release
sudo cp target/release/hyperlight-unikraft /usr/local/bin/
```

### 3. Run an Example

Each example has a Makefile that handles building and running:

```bash
# C example (fastest to build)
cd examples/helloworld-c
make all

# Python example
cd examples/python
make all

# Go example
cd examples/go
make all
```

The `make all` command will:
1. Build the Unikraft kernel with kraft
2. Build the application rootfs (compile binary or extract from Docker)
3. Run the application on Hyperlight

## Examples

| Example | Binary | Notes |
|---------|--------|-------|
| `helloworld-c` | Static PIE C binary | Compiled with `musl-gcc` |
| `rust` | Static PIE Rust binary | Compiled with `rustc --target x86_64-unknown-linux-musl` |
| `python` | CPython 3.12 | Rootfs from Docker, script passed via cmdline |
| `go` | Static PIE Go binary | Compiled with musl via Docker for CGO support |
| `nodejs` | Node.js 21 | Rootfs from Alpine, script passed via cmdline |

### Running with Arguments

For interpreted languages, pass the script path after `--`:

```bash
# Python
hyperlight-unikraft kernel --initrd python.cpio --memory 256Mi -- /script.py arg1 arg2

# Node.js
hyperlight-unikraft kernel --initrd node.cpio --memory 512Mi -- /app/server.js --port 8080
```

## CLI Options

```
hyperlight-unikraft [OPTIONS] <KERNEL> [-- <APP_ARGS>...]

Arguments:
  <KERNEL>       Path to the Unikraft kernel binary
  <APP_ARGS>...  Arguments passed to the application (after --)

Options:
  -m, --memory <MEMORY>  Memory allocation [default: 512Mi]
      --stack <STACK>    Stack size [default: 8Mi]
      --initrd <CPIO>    Path to initrd/rootfs CPIO archive
  -q, --quiet            Suppress kernel output
  -h, --help             Print help
  -V, --version          Print version
```

## Project Structure

```
hyperlight-unikraft/
├── host/                    # Rust CLI host
│   ├── Cargo.toml
│   └── src/main.rs
├── examples/
│   ├── helloworld-c/       # C example (musl-gcc)
│   ├── rust/               # Rust example (musl)
│   ├── python/             # Python 3.12 example
│   ├── go/                 # Go example (Docker + musl)
│   └── nodejs/             # Node.js 21 example
└── README.md
```

## Dependencies

This project requires the following forked repositories with Hyperlight platform support:

| Repository | Branch | Description |
|------------|--------|-------------|
| [danbugs/hyperlight](https://github.com/danbugs/hyperlight) | `hyperlight-platform` | Hyperlight with hw-interrupts feature |
| [danbugs/unikraft](https://github.com/danbugs/unikraft) | `hyperlight-platform` | Unikraft with Hyperlight platform |
| [danbugs/app-elfloader](https://github.com/danbugs/app-elfloader) | `hyperlight-platform` | ELF loader with PAGE_ALIGN fixes |
| [danbugs/kraftkit](https://github.com/danbugs/kraftkit) | `hyperlight-platform` | Kraft with Hyperlight machine driver |

The `kraft.yaml` files in the examples already reference the Unikraft and app-elfloader forks.
The host's `Cargo.toml` references the Hyperlight fork.
