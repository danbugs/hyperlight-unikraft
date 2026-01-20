# Node.js on Hyperlight Makefile
KERNEL := .unikraft/build/nodejs-hyperlight_hyperlight-x86_64
INITRD := node-initrd.cpio
MEMORY := 512Mi
SCRIPT ?= /app/hello.js

.PHONY: all build rootfs run run-only benchmark clean

all: build rootfs run-only

build:
	yes | kraft-hyperlight build --plat hyperlight --arch x86_64

rootfs: Dockerfile hello.js
	docker build --platform linux/amd64 -t nodejs-hyperlight-rootfs .
	docker create --name nodejs-tmp nodejs-hyperlight-rootfs /bin/true || true
	mkdir -p rootfs
	docker export nodejs-tmp | tar -C rootfs -xf -
	docker rm nodejs-tmp
	cd rootfs && find . | cpio -o -H newc > ../$(INITRD)

run-only:
	@hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT)

run: build rootfs run-only

benchmark: build rootfs
	@echo "=== Node.js on Hyperlight Benchmark ==="
	@echo "Kernel: $(KERNEL)"
	@echo "Initrd: $(INITRD)"
	@echo "Memory: $(MEMORY)"
	@echo ""
	@echo "--- Single run (with output) ---"
	@/usr/bin/time -f "real %E\nuser %U\nsys %S" hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT)
	@echo ""
	@echo "--- 10 runs ---"
	@total=0; \
	for i in 1 2 3 4 5 6 7 8 9 10; do \
		t=$$(/usr/bin/time -f "%e" hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) -- $(SCRIPT) 2>&1 | tail -1); \
		ms=$$(echo "$$t * 1000 / 1" | bc); \
		echo "  Run $$i: $${ms}ms"; \
		total=$$((total + ms)); \
	done; \
	avg=$$((total / 10)); \
	echo ""; \
	echo "Average: $${avg}ms"

clean:
	rm -rf .unikraft rootfs $(INITRD)
	-docker rmi nodejs-hyperlight-rootfs 2>/dev/null
	@echo "Cleaned"
