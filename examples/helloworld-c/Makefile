# Hello World C on Hyperlight - Makefile
#
# Usage:
#   make build    - Build the Unikraft kernel
#   make rootfs   - Compile C binary and create initrd.cpio
#   make run      - Build everything and run
#   make benchmark - Run performance benchmark
#   make clean    - Clean build artifacts

KERNEL := .unikraft/build/helloworld-hyperlight_hyperlight-x86_64
INITRD := hello-initrd.cpio
MEMORY := 32Mi

.PHONY: all build rootfs run run-only benchmark clean

all: build rootfs run-only

# Build the Unikraft kernel with kraft
build:
	yes | kraft-hyperlight build --plat hyperlight --arch x86_64

# Compile C binary as static PIE and create initrd
rootfs: hello.c
	musl-gcc -static-pie -fPIE -o hello hello.c
	mkdir -p rootfs/bin
	cp hello rootfs/bin/
	cd rootfs && find . | cpio -o -H newc > ../$(INITRD)
	@echo "Created $(INITRD)"

# Build and run
run: build rootfs run-only

# Run without rebuilding
run-only:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY)

# Performance benchmark
benchmark: build rootfs
	@echo "=== Hello C on Hyperlight Benchmark ==="
	@echo "Kernel: $(KERNEL)"
	@echo "Initrd: $(INITRD)"
	@echo "Memory: $(MEMORY)"
	@echo ""
	@echo "--- Single run (with output) ---"
	@/usr/bin/time -f "real %e\nuser %U\nsys %S" hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) 2>&1
	@echo ""
	@echo "--- 10 runs ---"
	@total=0; \
	for i in 1 2 3 4 5 6 7 8 9 10; do \
		t=$$(/usr/bin/time -f "%e" hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) 2>&1 | tail -1); \
		ms=$$(echo "$$t * 1000 / 1" | bc); \
		echo "  Run $$i: $${ms}ms"; \
		total=$$((total + ms)); \
	done; \
	avg=$$((total / 10)); \
	echo ""; \
	echo "Average: $${avg}ms"

clean:
	rm -rf .unikraft/build rootfs hello $(INITRD)
	@echo "Cleaned build artifacts"
