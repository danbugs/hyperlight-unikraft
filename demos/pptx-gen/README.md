# PPTX Generator Demo

Generates PowerPoint files by running LLM-generated Python code inside a Hyperlight-Unikraft micro-VM.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  User prompt: "Create a presentation about Hyperlight"          │
└────────────────────────────┬────────────────────────────────────┘
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│  OpenAI API → generates python-pptx code                        │
└────────────────────────────┬────────────────────────────────────┘
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│  Hyperlight-Unikraft Micro-VM                                   │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Unikraft kernel + Python 3.12 + python-pptx              │  │
│  │  Executes code → outputs PPTX as base64 to stdout         │  │
│  └───────────────────────────────────────────────────────────┘  │
└────────────────────────────┬────────────────────────────────────┘
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│  Host decodes base64 → presentation.pptx                        │
└─────────────────────────────────────────────────────────────────┘
```

## Prerequisites

- Linux with KVM (`/dev/kvm` with r/w access)
- Rust 1.75+
- Docker
- `kraft-hyperlight` CLI
- `cpio` (`sudo apt install cpio`)
- OpenAI API key

## Quick Start

### 1. Set up your API key

```bash
cp .env.example .env
# Add your OpenAI API key
```

### 2. Build

```bash
make all
```

### 3. Run

```bash
./target/release/hyperlight-pptx-gen \
    --prompt "Create a 5-slide presentation about cloud security"
```

## CLI Options

```
hyperlight-pptx-gen [OPTIONS] --prompt <PROMPT>

  -p, --prompt <PROMPT>    Presentation description
  -o, --output <OUTPUT>    Output path [default: presentation.pptx]
      --kernel <KERNEL>    Unikraft kernel [default: assets/kernel]
      --rootfs <ROOTFS>    Rootfs CPIO [default: assets/rootfs.cpio]
      --memory <MEMORY>    VM memory [default: 512Mi]
      --model <MODEL>      OpenAI model [default: gpt-4o]
      --dry-run            Print generated code without executing
  -h, --help
```

## Examples

```bash
# Simple
hyperlight-pptx-gen -p "3 slides about Rust"

# Custom output
hyperlight-pptx-gen -p "Quarterly review" -o q4-review.pptx

# Dry run (see generated code)
hyperlight-pptx-gen -p "Test" --dry-run
```

## Security

The generated Python code runs inside an isolated micro-VM:
- No host filesystem access
- No network access
- Ephemeral (destroyed after execution)
- Resource constrained

## Troubleshooting

**"hyperlight-unikraft not found"**: Install from `../../host`:
```bash
cargo build --release && sudo cp target/release/hyperlight-unikraft /usr/local/bin/
```

**"Kernel not found"**: Run `make assets`

**"OPENAI_API_KEY not set"**: Create `.env`:
```bash
echo "OPENAI_API_KEY=sk-your-key" > .env
```

**VM fails**: Check KVM access:
```bash
ls -la /dev/kvm
```

## How It Works

1. CLI sends prompt to OpenAI with instructions to generate python-pptx code
2. Generated code is injected into the rootfs CPIO
3. hyperlight-unikraft boots the kernel with the modified rootfs
4. Python executes, saves PPTX, prints it as base64 to stdout
5. Host extracts base64 and saves as .pptx

## License

See repository root.
