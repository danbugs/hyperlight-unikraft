# Shell (BusyBox) on Hyperlight - Build and Run
#
# Usage:
#   make rootfs      - Build BusyBox rootfs Docker image and export to CPIO
#   make build       - Build Unikraft kernel with kraft
#   make run         - Run with hyperlight-unikraft
#   make benchmark   - Benchmark cold start time
#   make all         - Build everything and run
#   make clean       - Clean up
#
# Examples:
#   make run                          # Run demo.sh
#   make run SCRIPT="/demo.sh"        # Run demo.sh (explicit)
#   make run SCRIPT="-c 'echo hi'"    # Run inline command

IMAGE_NAME = shell-hyperlight-rootfs
CONTAINER_NAME = $(IMAGE_NAME)
KERNEL = .unikraft/build/shell-hyperlight_hyperlight-x86_64
INITRD = initrd.cpio
MEMORY ?= 64Mi
BENCHMARK_RUNS ?= 10
SCRIPT ?= /demo.sh

.PHONY: all build rootfs run run-only run-quiet benchmark clean

all: build rootfs run

# Build the Unikraft kernel using kraft
build:
	kraft-hyperlight build --plat hyperlight --arch x86_64

# Build BusyBox Docker image and export rootfs to CPIO
rootfs: $(INITRD)

$(INITRD): Dockerfile demo.sh
	@echo "Building BusyBox Docker image..."
	docker build --platform linux/amd64 -f Dockerfile -t $(IMAGE_NAME) .
	@echo "Exporting rootfs..."
	-docker rm -f $(CONTAINER_NAME) 2>/dev/null
	docker create --name $(CONTAINER_NAME) $(IMAGE_NAME) /bin/true || true
	rm -rf rootfs && mkdir -p rootfs
	docker export $(CONTAINER_NAME) | tar -x -C rootfs
	docker rm -f $(CONTAINER_NAME)
	@echo "Adding demo.sh..."
	cp demo.sh rootfs/demo.sh
	@echo "Creating CPIO archive..."
	cd rootfs && find . | cpio -o -H newc > ../$(INITRD)
	@echo "Created $(INITRD)"
	rm -rf rootfs

# Run with hyperlight-unikraft
run: $(KERNEL) $(INITRD)
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT)

# Run without rebuilding (with output)
run-only:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT)

# Run in quiet mode
run-quiet:
	hyperlight-unikraft -q $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT)

# Benchmark cold start time
benchmark: $(KERNEL) $(INITRD)
	@echo "=== Shell on Hyperlight Benchmark ==="
	@echo "Kernel: $(KERNEL)"
	@echo "Initrd: $(INITRD)"
	@echo "Memory: $(MEMORY)"
	@echo "Script: $(SCRIPT)"
	@echo ""
	@echo "--- Single run (with output) ---"
	@time -p hyperlight-unikraft -q $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT) 2>&1
	@echo ""
	@echo "--- $(BENCHMARK_RUNS) runs ---"
	@total=0; \
	for i in $$(seq 1 $(BENCHMARK_RUNS)); do \
		start=$$(date +%s%N); \
		hyperlight-unikraft -q $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT) 2>/dev/null; \
		end=$$(date +%s%N); \
		elapsed=$$((end - start)); \
		ms=$$((elapsed / 1000000)); \
		echo "  Run $$i: $${ms}ms"; \
		total=$$((total + elapsed)); \
	done; \
	avg=$$((total / $(BENCHMARK_RUNS) / 1000000)); \
	echo ""; \
	echo "Average: $${avg}ms"

# Clean up
clean:
	rm -rf rootfs $(INITRD)
	-docker rm -f $(CONTAINER_NAME) 2>/dev/null
	-docker rmi $(IMAGE_NAME) 2>/dev/null
	rm -rf .unikraft
