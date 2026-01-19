# Go on Hyperlight Makefile
#
# Usage:
#   make build   - Build Unikraft kernel
#   make rootfs  - Build Go binary via Docker and create initrd
#   make run     - Build and run
#   make clean   - Clean up

KERNEL := .unikraft/build/go-hyperlight_hyperlight-x86_64
INITRD := hello-initrd.cpio
MEMORY := 64Mi
IMAGE_NAME := go-hyperlight-builder
CONTAINER_NAME := $(IMAGE_NAME)

.PHONY: build rootfs run run-only benchmark clean

build:
	@echo "=== Building Unikraft kernel ==="
	yes | kraft-hyperlight build --plat hyperlight --arch x86_64

rootfs: Dockerfile hello.go
	@echo "=== Building Go binary via Docker (static PIE) ==="
	docker build --platform linux/amd64 -t $(IMAGE_NAME) .
	@echo "=== Extracting rootfs ==="
	-docker rm -f $(CONTAINER_NAME) 2>/dev/null
	docker create --name $(CONTAINER_NAME) $(IMAGE_NAME) /bin/true
	rm -rf rootfs && mkdir -p rootfs
	docker export $(CONTAINER_NAME) | tar -x -C rootfs
	docker rm -f $(CONTAINER_NAME)
	@echo "=== Creating initrd.cpio ==="
	cd rootfs && find . | cpio -o -H newc > ../$(INITRD)
	@echo "Created $(INITRD)"
	rm -rf rootfs

run-only:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- hello

run: build rootfs run-only

benchmark: build rootfs
	@echo "=== Go on Hyperlight Benchmark ==="
	@echo "Kernel: $(KERNEL)"
	@echo "Initrd: $(INITRD)"
	@echo "Memory: $(MEMORY)"
	@echo ""
	@echo "--- Single run (with output) ---"
	@/usr/bin/time -f "real %E\nuser %U\nsys %S" hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- hello
	@echo ""
	@echo "--- 10 runs ---"
	@total=0; \
	for i in 1 2 3 4 5 6 7 8 9 10; do \
		t=$$(/usr/bin/time -f "%e" hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- hello 2>&1 | tail -1); \
		ms=$$(echo "$$t * 1000 / 1" | bc); \
		echo "  Run $$i: $${ms}ms"; \
		total=$$((total + ms)); \
	done; \
	avg=$$((total / 10)); \
	echo ""; \
	echo "Average: $${avg}ms"

clean:
	rm -rf .unikraft rootfs hello $(INITRD)
	-docker rm -f $(CONTAINER_NAME) 2>/dev/null
	-docker rmi $(IMAGE_NAME) 2>/dev/null
	@echo "Cleaned"
