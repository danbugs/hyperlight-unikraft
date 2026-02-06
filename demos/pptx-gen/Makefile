# Hyperlight PPTX Generator - Build and Run
#
# Usage:
#   make assets      - Build pre-requisite kernel and rootfs
#   make build       - Build the Rust CLI
#   make run         - Run with a sample prompt
#   make clean       - Clean up

KERNEL_DIR = ../../examples/python
ASSETS_DIR = assets
KERNEL = .unikraft/build/python-pptx-hyperlight_hyperlight-x86_64

.PHONY: all build assets run clean help

all: assets build

help:
	@echo "Hyperlight PPTX Generator"
	@echo ""
	@echo "Usage:"
	@echo "  make assets    - Build Unikraft kernel and Python rootfs with python-pptx"
	@echo "  make build     - Build the Rust CLI tool"
	@echo "  make run       - Run with a sample prompt"
	@echo "  make demo      - Run a demo presentation generation"
	@echo "  make clean     - Clean build artifacts"
	@echo ""

# Build the Rust CLI
build:
	cargo build --release

# Build pre-requisite assets (kernel + rootfs with python-pptx)
assets: $(ASSETS_DIR)/kernel $(ASSETS_DIR)/rootfs.cpio

# Build Unikraft kernel
$(ASSETS_DIR)/kernel: kraft.yaml
	@echo "Building Unikraft kernel..."
	mkdir -p $(ASSETS_DIR)
	kraft-hyperlight build --plat hyperlight --arch x86_64
	cp $(KERNEL) $(ASSETS_DIR)/kernel
	@echo "Kernel built: $(ASSETS_DIR)/kernel"

# Build rootfs with Python + python-pptx
$(ASSETS_DIR)/rootfs.cpio: Dockerfile.rootfs
	@echo "Building Python rootfs with python-pptx..."
	mkdir -p $(ASSETS_DIR)
	docker build --platform linux/amd64 -f Dockerfile.rootfs -t pptx-gen-rootfs .
	@echo "Exporting rootfs..."
	-docker rm -f pptx-gen-rootfs-container 2>/dev/null
	docker create --name pptx-gen-rootfs-container pptx-gen-rootfs /bin/true || true
	rm -rf rootfs && mkdir -p rootfs
	docker export pptx-gen-rootfs-container | tar -x -C rootfs
	docker rm -f pptx-gen-rootfs-container
	@echo "Creating CPIO archive..."
	cd rootfs && find . | cpio -o -H newc > ../$(ASSETS_DIR)/rootfs.cpio
	rm -rf rootfs
	@echo "Rootfs built: $(ASSETS_DIR)/rootfs.cpio"

# Run with a sample prompt
run: build assets
	./target/release/hyperlight-pptx-gen \
		--prompt "Create a 3-slide presentation about cats"

# Show code generation without execution
dry-run: build
	./target/release/hyperlight-pptx-gen \
		--prompt "Create a 5-slide presentation about cloud security" \
		--dry-run

# Clean build artifacts
clean:
	cargo clean
	rm -rf $(ASSETS_DIR)
	rm -rf rootfs
	rm -f *.pptx
