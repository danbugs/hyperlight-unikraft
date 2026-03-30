# Go on Hyperlight - Build and Run
#
# Usage:
#   make rootfs      - Build Go rootfs Docker image and export to CPIO
#   make build       - Build Unikraft kernel with kraft
#   make run         - Build everything and run
#   make benchmark   - Benchmark cold start time
#   make all         - Build everything and run
#   make rebuild     - Clean everything and rebuild from scratch
#   make clean       - Clean up

IMAGE_NAME = go-hyperlight-rootfs
CONTAINER_NAME = $(IMAGE_NAME)
KERNEL = .unikraft/build/go-hyperlight_hyperlight-x86_64
INITRD = hello-initrd.cpio
MEMORY ?= 16Mi
BENCHMARK_RUNS ?= 10
KRAFT_NAME := $(shell grep '^name:' kraft.yaml 2>/dev/null | awk '{print $$2}')
UNIKRAFT_BRANCH := $(shell grep 'version:' kraft.yaml | head -1 | awk '{print $$2}')

.PHONY: all build rootfs run run-only benchmark rebuild clean

all: build rootfs run

build:
	@rm -f .config.$(KRAFT_NAME)_hyperlight-x86_64 .config.$(KRAFT_NAME)_hyperlight-x86_64.old
	-kraft-hyperlight build --plat hyperlight --arch x86_64

fix-sources:
	@mkdir -p .unikraft/apps .unikraft/libs
	@rm -rf .unikraft/unikraft .unikraft/apps/elfloader
	@git clone --branch $(UNIKRAFT_BRANCH) --depth 1 https://github.com/danbugs/unikraft.git .unikraft/unikraft 2>/dev/null || true
	@git clone --branch $(UNIKRAFT_BRANCH) --depth 1 https://github.com/danbugs/app-elfloader.git .unikraft/apps/elfloader 2>/dev/null || true
	@git clone --branch staging --depth 1 https://github.com/unikraft/lib-libelf.git .unikraft/libs/libelf 2>/dev/null || true

rebuild: clean
	rm -f $(HOME)/.local/share/kraftkit/sources/unikraft/*.tar.gz
	rm -f $(HOME)/.local/share/kraftkit/sources/apps/elfloader/*.tar.gz
	-kraft-hyperlight build --plat hyperlight --arch x86_64
	$(MAKE) fix-sources
	rm -rf .unikraft/build
	$(MAKE) build

rootfs: $(INITRD)

$(INITRD): Dockerfile hello.go
	@echo "Building Go Docker image..."
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
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- hello

run-only:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- hello

run-5:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) --repeat 4 -- hello

benchmark: $(KERNEL) $(INITRD)
	@echo "Go on Hyperlight Benchmark"
	@echo "Kernel: $(KERNEL)"
	@echo "Initrd: $(INITRD)"
	@echo "Memory: $(MEMORY)"
	@echo ""
	@echo "Single run (with output):"
	@time -p hyperlight-unikraft -q $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- hello 2>&1
	@echo ""
	@echo "$(BENCHMARK_RUNS) runs:"
	@total=0; \
	for i in $$(seq 1 $(BENCHMARK_RUNS)); do \
		start=$$(date +%s%N); \
		hyperlight-unikraft -q $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- hello 2>/dev/null; \
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
		rm -f .config.$(KRAFT_NAME)_hyperlight-x86_64
	rm -f .config.$(KRAFT_NAME)_hyperlight-x86_64.old
