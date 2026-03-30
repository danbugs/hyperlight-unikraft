# Python Tool Calling on Hyperlight - Build and Run
#
# Usage:
#   make rootfs      - Build Python rootfs Docker image and export to CPIO
#   make build       - Build Unikraft kernel with kraft
#   make run         - Run with hyperlight-unikraft (tools enabled)
#   make all         - Build everything and run
#   make clean       - Clean up

PYTHON_VERSION ?= 3.12.0
IMAGE_NAME = python-tools-hyperlight-rootfs
CONTAINER_NAME = $(IMAGE_NAME)
KERNEL = .unikraft/build/python-tools-hyperlight_hyperlight-x86_64
INITRD = initrd.cpio
MEMORY ?= 96Mi
SCRIPT ?= /test_tools.py
KRAFT_NAME := $(shell grep '^name:' kraft.yaml 2>/dev/null | awk '{print $$2}')
UNIKRAFT_BRANCH := $(shell grep 'version:' kraft.yaml | head -1 | awk '{print $$2}')

.PHONY: all build rootfs run run-only clean

all: build rootfs run

fix-sources:
	@rm -rf .unikraft/unikraft .unikraft/apps/elfloader
	@git clone --branch $(UNIKRAFT_BRANCH) --depth 1 https://github.com/danbugs/unikraft.git .unikraft/unikraft 2>/dev/null || true
	@git clone --branch $(UNIKRAFT_BRANCH) --depth 1 https://github.com/danbugs/app-elfloader.git .unikraft/apps/elfloader 2>/dev/null || true

rebuild: clean
	rm -f $(HOME)/.local/share/kraftkit/sources/unikraft/*.tar.gz
	rm -f $(HOME)/.local/share/kraftkit/sources/apps/elfloader/*.tar.gz
	kraft-hyperlight build --plat hyperlight --arch x86_64
	$(MAKE) fix-sources
	rm -rf .unikraft/build
	$(MAKE) build

# Build the Unikraft kernel using kraft
build:
	kraft-hyperlight build --plat hyperlight --arch x86_64

# Build Python Docker image and export rootfs to CPIO
rootfs: $(INITRD)

$(INITRD): Dockerfile hyperlight.py test_tools.py
	@echo "Building Python Docker image..."
	docker build --platform linux/amd64 -f Dockerfile --build-arg PYTHON_VERSION=$(PYTHON_VERSION) -t $(IMAGE_NAME) .
	@echo "Exporting rootfs..."
	-docker rm -f $(CONTAINER_NAME) 2>/dev/null
	docker create --name $(CONTAINER_NAME) $(IMAGE_NAME) /bin/true || true
	rm -rf rootfs && mkdir -p rootfs
	docker export $(CONTAINER_NAME) | tar -x -C rootfs
	docker rm -f $(CONTAINER_NAME)
	@echo "Adding test script..."
	cp test_tools.py rootfs/test_tools.py
	@echo "Creating CPIO archive..."
	cd rootfs && find . | cpio -o -H newc > ../$(INITRD)
	@echo "Created $(INITRD)"
	rm -rf rootfs

# Run with hyperlight-unikraft (tools enabled)
run: $(KERNEL) $(INITRD)
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) --enable-tools -- $(SCRIPT)

# Run without rebuilding
run-only:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) --enable-tools -- $(SCRIPT)

run-5:
	hyperlight-unikraft $(KERNEL) --initrd $(INITRD) --memory $(MEMORY) --repeat 4 --enable-tools -- $(SCRIPT)		

# Clean up
clean:
	rm -rf rootfs $(INITRD)
	-docker rm -f $(CONTAINER_NAME) 2>/dev/null
	-docker rmi $(IMAGE_NAME) 2>/dev/null
	rm -rf .unikraft
	rm -f .config.$(KRAFT_NAME)_hyperlight-x86_64
	rm -f .config.$(KRAFT_NAME)_hyperlight-x86_64.old