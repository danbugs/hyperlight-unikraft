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
MEMORY ?= 256Mi
SCRIPT ?= /test_tools.py

.PHONY: all build rootfs run run-only clean

all: build rootfs run

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

# Clean up
clean:
	rm -rf rootfs $(INITRD)
	-docker rm -f $(CONTAINER_NAME) 2>/dev/null
	-docker rmi $(IMAGE_NAME) 2>/dev/null
	rm -rf .unikraft
