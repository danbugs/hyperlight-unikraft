# Node.js on Hyperlight - Build and Run
#
# Usage:
#   make rootfs      - Build Node.js rootfs Docker image and export to CPIO
#   make build       - Build Unikraft kernel with kraft
#   make run         - Build everything and run
#   make benchmark   - Benchmark cold start time
#   make all         - Build everything and run
#   make clean       - Clean up

IMAGE_NAME = nodejs-hyperlight-rootfs
CONTAINER_NAME = $(IMAGE_NAME)
KERNEL = .unikraft/build/nodejs-hyperlight_hyperlight-x86_64
INITRD = node-initrd.cpio
MEMORY ?= 512Mi
BENCHMARK_RUNS ?= 10
SCRIPT ?= /app/hello.js

.PHONY: all build rootfs run run-only benchmark clean

all: build rootfs run

build:
	kraft-hyperlight build --plat hyperlight --arch x86_64

rootfs: $(INITRD)

$(INITRD): Dockerfile hello.js
	@echo "Building Node.js Docker image..."
	docker build --platform linux/amd64 -t $(IMAGE_NAME) .
	@echo "Exporting rootfs..."
	-docker rm -f $(CONTAINER_NAME) 2>/dev/null
	docker create --name $(CONTAINER_NAME) $(IMAGE_NAME) /bin/true || true
	rm -rf rootfs && mkdir -p rootfs
	docker export $(CONTAINER_NAME) | tar -x -C rootfs
	docker rm -f $(CONTAINER_NAME)
	@echo "Creating CPIO archive..."
	cd rootfs && find . | cpio -o -H newc > ../$(INITRD)
	@echo "Created $(INITRD)"
	rm -rf rootfs

run: $(KERNEL) $(INITRD)
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT)

run-only:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT)

benchmark: $(KERNEL) $(INITRD)
	@echo "Node.js on Hyperlight Benchmark"
	@echo "Kernel: $(KERNEL)"
	@echo "Initrd: $(INITRD)"
	@echo "Memory: $(MEMORY)"
	@echo "Script: $(SCRIPT)"
	@echo ""
	@echo "Single run (with output):"
	@time -p hyperlight-unikraft -q $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT) 2>&1
	@echo ""
	@echo "$(BENCHMARK_RUNS) runs:"
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

clean:
	rm -rf rootfs $(INITRD)
	-docker rm -f $(CONTAINER_NAME) 2>/dev/null
	-docker rmi $(IMAGE_NAME) 2>/dev/null
	rm -rf .unikraft
