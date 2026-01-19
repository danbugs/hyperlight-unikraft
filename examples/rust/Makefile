# Rust on Hyperlight Makefile
KERNEL := .unikraft/build/rust-hyperlight_hyperlight-x86_64
INITRD := hello-initrd.cpio
MEMORY := 32Mi

.PHONY: build rootfs run run-only benchmark clean

build:
	@echo "=== Building Unikraft kernel ==="
	yes | kraft-hyperlight build --plat hyperlight --arch x86_64

rootfs: hello.rs
	@echo "=== Compiling Rust binary (static PIE with musl) ==="
	rustc --target x86_64-unknown-linux-musl -C target-feature=+crt-static -C relocation-model=pie -o hello hello.rs
	@echo "=== Creating initrd.cpio ==="
	mkdir -p rootfs/bin
	cp hello rootfs/bin/
	cd rootfs && find . | cpio -o -H newc > ../$(INITRD)
	@echo "Created $(INITRD)"

run-only:
	@hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY)

run: build rootfs run-only

benchmark: build rootfs
	@echo "=== Rust on Hyperlight Benchmark ==="
	@echo "Kernel: $(KERNEL)"
	@echo "Initrd: $(INITRD)"
	@echo "Memory: $(MEMORY)"
	@echo ""
	@echo "--- Single run (with output) ---"
	@/usr/bin/time -f "real %E\nuser %U\nsys %S" hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY)
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
	rm -rf .unikraft rootfs hello $(INITRD)
	@echo "Cleaned"
