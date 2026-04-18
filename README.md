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

Common on both Linux and Windows:

- [Rust](https://rustup.rs/) 1.89+
- [Docker](https://www.docker.com/) (builds the rootfs CPIO archives)
- [`just`](https://github.com/casey/just) (build runner — replaces Make)

Linux-only (needed to build Unikraft kernels locally):

- KVM (`/dev/kvm` readable/writable)
- Go 1.25+ (builds `kraft-hyperlight`)

Windows-only:

- Windows Hypervisor Platform (`Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform`; reboot)
- Developer Mode enabled (Settings → For developers → Developer Mode)
- Kernels are pulled pre-built from GHCR; `kraft-hyperlight` is not required.

## Setup

### Linux — from scratch

```bash
# 1. Toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo install just

# 2. kraft-hyperlight (builds Unikraft kernels)
git clone --branch hyperlight-platform https://github.com/danbugs/kraftkit.git
cd kraftkit && go build -o kraft-hyperlight ./cmd/kraft
sudo mv kraft-hyperlight /usr/local/bin/ && cd ..

# 3. This repo + host CLI
git clone https://github.com/danbugs/hyperlight-unikraft.git
cd hyperlight-unikraft/host
cargo build --release
sudo cp target/release/hyperlight-unikraft /usr/local/bin/
cd ..

# 4. Run any example
cd examples/helloworld-c
just build      # build the Unikraft kernel with kraft-hyperlight
just rootfs     # build the rootfs CPIO via Docker
just run
```

### Windows — from scratch

```powershell
# 1. Toolchain
# Install Rust via https://www.rust-lang.org/tools/install
cargo install just

# 2. This repo + host CLI
git clone https://github.com/danbugs/hyperlight-unikraft.git
cd hyperlight-unikraft\host
cargo build --release
Copy-Item target\release\hyperlight-unikraft.exe $env:USERPROFILE\.cargo\bin\ -Force
cd ..

# 3. Run any example (kernel pulled from GHCR)
cd examples\helloworld-c
just build      # docker pull ghcr.io/danbugs/hyperlight-unikraft/helloworld-c-kernel
just rootfs     # docker build + extract CPIO
just run
```

### What each recipe does

| Recipe | Linux | Windows |
|--------|-------|---------|
| `just build` | `kraft-hyperlight build` | `docker pull` the pre-built kernel from GHCR |
| `just rootfs` | `docker build --target cpio` + extract the CPIO | same |
| `just run` | `hyperlight-unikraft <kernel> --initrd ...` | same |
| `just clean` | remove `.unikraft/` and the CPIO | same |

## Examples

| Example | Binary | Notes |
|---------|--------|-------|
| `helloworld-c` | Static PIE C binary | Compiled with `musl-gcc` |
| `rust` | Static PIE Rust binary | Compiled with `rustc --target x86_64-unknown-linux-musl` |
| `python` | CPython 3.12 | Rootfs from Docker, script passed via cmdline |
| `go` | Static PIE Go binary | Compiled with musl via Docker for CGO support |
| `nodejs` | Node.js 21 | Rootfs from Alpine, script passed via cmdline |
| `hostfs-posix-c` | C + unmodified POSIX | `open`/`read`/`write`/`mkdir` against `/host`, forwarded by `lib/hostfs` |
| `hostfs-posix-py` | Python + stdlib | Same as `hostfs-posix-c` using `open()`/`os.mkdir`/`os.stat` |

### Host filesystem sandbox

`--mount HOST_DIR[:GUEST_PATH]` preopens a host directory for the guest:

```bash
# Default: guest-visible at /host
hyperlight-unikraft kernel --initrd app.cpio --mount ./work

# Custom guest mount point
hyperlight-unikraft kernel --initrd app.cpio --mount ./work:/data
```

`lib/hostfs` in the guest auto-mounts `HOST_DIR` at `GUEST_PATH` (default
`/host`); unmodified POSIX calls (`open`, `read`, `write`, `stat`,
`mkdir`, `truncate`, …) are forwarded by the VFS driver to the host's
`FsSandbox` tool handlers. The guest mount point is advertised runtime
via an `HLHSMNT` TLV in init_data, so one kernel build can serve
different mount points. Reserved kernel dirs (`/`, `/bin`, `/dev`,
`/proc`, `/sys`, `/usr`) are refused to avoid shadowing the initrd.

Every path the guest sends is resolved relative to `HOST_DIR` and any
escape (via `..` or symlinks) is rejected host-side.

Known limitation: `opendir`/`readdir` don't work yet (see
[lib/hostfs/README.md](https://github.com/danbugs/unikraft/blob/hyperlight-platform-v0.13.1-rebased/lib/hostfs/README.md)). Stat and enumerate known paths instead.

### Running ad-hoc code (no initrd rebuild)

`--exec CODE` / `-e CODE` feeds a snippet to the guest interpreter as
`-c CODE`. The host handles all the argparse-escape quoting internally,
so you can pass arbitrary whitespace, quotes, and newlines without
wrapping:

```bash
hyperlight-unikraft python-kernel --initrd python.cpio --memory 96Mi \
    --exec 'for i in range(3): print(i * i)'
```

Works for any interpreter that treats `-c` as "run the next arg as
code" — CPython, `sh`, etc. `node -e` works identically with `-e`.

`examples/hostfs-posix-py` wraps it in two Justfile recipes:

```bash
just exec "print('hi'); print(2 + 2)"
just run-file path/to/myscript.py   # file's contents → --exec
```

No `--mount` involved. No `/host/…` path contract. The host just passes
argv.

#### Passing extra script arguments

`--exec` and positional `-- args` are mutually exclusive (clap enforces
it at parse time) — they both populate argv, so letting both through
would silently lose one. If you need inline code *plus* extra `sys.argv`
arguments, drop back to the raw `--` form and do the quoting yourself:

```bash
hyperlight-unikraft python-kernel --initrd python.cpio --memory 96Mi \
    -- -c '"import sys; print(sys.argv[1:])"' alpha beta gamma
# => ['alpha', 'beta', 'gamma']
```

The inner `-c` payload is wrapped in outer double-quotes so `uk_argparse`
preserves whitespace, with internal quotes backslash-escaped. Anything
after is plain argv.

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
