# Hello World C on Hyperlight - Build and Run
#
# Usage:
#   make rootfs      - Build C binary and create initrd.cpio
#   make build       - Build Unikraft kernel with kraft
#   make run         - Build everything and run
#   make benchmark   - Benchmark cold start time
#   make all         - Build everything and run
#   make rebuild     - Clean everything and rebuild from scratch
#   make clean       - Clean up

KERNEL = .unikraft/build/helloworld-hyperlight_hyperlight-x86_64
INITRD = hello-initrd.cpio
MEMORY ?= 4Mi
BENCHMARK_RUNS ?= 10
KRAFT_NAME := $(shell grep '^name:' kraft.yaml 2>/dev/null | awk '{print $$2}')

.PHONY: all build rootfs run run-only benchmark rebuild clean

all: build rootfs run

build:
	kraft-hyperlight build --plat hyperlight --arch x86_64

rebuild: clean
	rm -f .config.$(KRAFT_NAME)_hyperlight-x86_64
	rm -f .config.$(KRAFT_NAME)_hyperlight-x86_64.old
	rm -f $(HOME)/.local/share/kraftkit/sources/unikraft/*.tar.gz
	rm -f $(HOME)/.local/share/kraftkit/sources/apps/elfloader/*.tar.gz
	kraft-hyperlight build --plat hyperlight --arch x86_64

rootfs: $(INITRD)

$(INITRD): hello.c
	@echo "Building C binary..."
	musl-gcc -static-pie -fPIE -fno-stack-protector -nostartfiles -o hello hello.c
	@echo "Creating rootfs..."
	rm -rf rootfs && mkdir -p rootfs/bin
	cp hello rootfs/bin/
	@echo "Creating CPIO archive..."
	cd rootfs && find . | cpio -o -H newc > ../$(INITRD)
	@echo "Created $(INITRD)"
	rm -rf rootfs

run: $(KERNEL) $(INITRD)
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY)

run-only:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY)

run-5:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) --repeat 5

benchmark: $(KERNEL) $(INITRD)
	@echo "Hello C on Hyperlight Benchmark"
	@echo "Kernel: $(KERNEL)"
	@echo "Initrd: $(INITRD)"
	@echo "Memory: $(MEMORY)"
	@echo ""
	@echo "Single run (with output):"
	@time -p hyperlight-unikraft -q $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) 2>&1
	@echo ""
	@echo "$(BENCHMARK_RUNS) runs:"
	@total=0; \
	for i in $$(seq 1 $(BENCHMARK_RUNS)); do \
		start=$$(date +%s%N); \
		hyperlight-unikraft -q $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) 2>/dev/null; \
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
	rm -rf rootfs $(INITRD) hello
	rm -rf .unikraft
