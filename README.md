# hyperlight-unikraft

Run Unikraft unikernels on [Hyperlight](https://github.com/hyperlight-dev/hyperlight), a lightweight Virtual Machine Manager (VMM) designed for embedded use within applications.

## Overview

This project provides:

1. **An embedded Hyperlight host** (`hyperlight-unikraft`) - A CLI tool that runs Unikraft kernels on Hyperlight
2. **Kraft configurations** - Ready-to-use configurations for building various applications (Python, Node.js, Go, Rust, C/C++)

## How It Works

```
┌─────────────────────────────────────────────────┐
│  Your Application (Rust, Python, Node.js, etc) │
├─────────────────────────────────────────────────┤
│  hyperlight-unikraft (embedded VMM)             │
├─────────────────────────────────────────────────┤
│  Hyperlight (hypervisor interface)              │
├─────────────────────────────────────────────────┤
│  KVM / MSHV                                     │
└─────────────────────────────────────────────────┘
```

The Unikraft kernel with the ELF loader acts as a bootloader that:
1. Extracts the CPIO initrd to a RAM filesystem
2. Loads the application ELF binary (Python, Node.js, etc.)
3. Executes it in the Hyperlight micro-VM

## Prerequisites

- Linux with KVM support (`/dev/kvm` with read/write access)
- [kraft](https://unikraft.org/docs/getting-started) CLI tool
- Docker (for extracting application rootfs)
- Rust toolchain (for building the host)

## Quick Start

### 1. Build the Host

```bash
cd host
cargo build --release
sudo cp target/release/hyperlight-unikraft /usr/local/bin/
```

### 2. Build a Unikraft Kernel (Python example)

```bash
cd examples/python
kraft build --plat hyperlight --arch x86_64
```

### 3. Get the Application Rootfs

```bash
# Extract Python rootfs from Docker image
docker run --rm unikraft.org/python:3.12 --rootfs python-initrd.cpio
```

### 4. Run

```bash
hyperlight-unikraft .unikraft/build/python-hyperlight_hyperlight-x86_64 \
  --initrd python-initrd.cpio \
  --memory 256Mi
```

## Examples

### Python

```bash
cd examples/python
kraft build --plat hyperlight --arch x86_64
docker run --rm unikraft.org/python:3.12 --rootfs python-initrd.cpio

hyperlight-unikraft .unikraft/build/python-hyperlight_hyperlight-x86_64 \
  --initrd python-initrd.cpio --memory 256Mi
```

### Node.js

```bash
cd examples/nodejs
kraft build --plat hyperlight --arch x86_64
docker run --rm unikraft.org/node:21 --rootfs node-initrd.cpio

hyperlight-unikraft .unikraft/build/nodejs-hyperlight_hyperlight-x86_64 \
  --initrd node-initrd.cpio --memory 512Mi
```

### Custom Hello World (C)

For a simple C program without the full Docker rootfs:

```bash
cd examples/helloworld-c
kraft build --plat hyperlight --arch x86_64

# Create minimal initrd with just your binary
mkdir -p rootfs/bin
cp my-hello /rootfs/bin/hello
cd rootfs && find . | cpio -o -H newc > ../hello-initrd.cpio

hyperlight-unikraft .unikraft/build/helloworld-hyperlight_hyperlight-x86_64 \
  --initrd hello-initrd.cpio --memory 64Mi
```

## CLI Options

```
hyperlight-unikraft [OPTIONS] <KERNEL>

Arguments:
  <KERNEL>  Path to the Unikraft kernel binary

Options:
  -m, --memory <MEMORY>  Memory allocation (e.g., 256Mi, 512Mi, 1Gi) [default: 512Mi]
      --stack <STACK>    Stack size (e.g., 8Mi) [default: 8Mi]
      --initrd <CPIO>    Path to initrd/rootfs CPIO archive
  -q, --quiet            Suppress kernel output
  -h, --help             Print help
  -V, --version          Print version
```

## Project Structure

```
hyperlight-unikraft/
├── host/                    # Embedded Hyperlight host
│   ├── Cargo.toml
│   └── src/main.rs
├── examples/
│   ├── python/             # Python kraft config
│   ├── nodejs/             # Node.js kraft config
│   ├── go/                 # Go kraft config
│   ├── rust/               # Rust kraft config
│   └── helloworld-c/       # Simple C example
└── README.md
```

## Dependencies

This project requires patches to:

1. **Unikraft** - Hyperlight platform support
2. **Hyperlight** - `init-paging` and `hw-interrupts` features enabled

See the patches directory for the required changes.

## License

MIT OR Apache-2.0
