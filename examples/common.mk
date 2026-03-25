# Common build rules for hyperlight-unikraft examples
#
# kraft-hyperlight needs two passes when building from scratch:
#   1. First run fetches sources and generates .config (may fail on TTY prompt)
#   2. Second run builds with --no-configure to skip Kconfig re-prompt
#
# Usage: include this from each example's Makefile, then define your own
# targets that depend on $(KERNEL).

# Extract the kraft.yaml app name for .config file cleanup
KRAFT_NAME := $(shell grep '^name:' kraft.yaml 2>/dev/null | awk '{print $$2}')

# Build kernel: fetch + configure first, then build with --no-configure
build:
	@kraft-hyperlight build --plat hyperlight --arch x86_64 2>/dev/null || true
	kraft-hyperlight build --plat hyperlight --arch x86_64 --no-fetch --no-update --no-configure

# Full clean rebuild from scratch (deletes .unikraft, .config files, and global cache)
rebuild: clean-cache
	$(MAKE) build

# Clean cached sources, build artifacts, and .config files
clean-cache:
	rm -rf .unikraft
	rm -f .config.$(KRAFT_NAME)_hyperlight-x86_64
	rm -f .config.$(KRAFT_NAME)_hyperlight-x86_64.old
	rm -f $(HOME)/.local/share/kraftkit/sources/unikraft/*.tar.gz
	rm -f $(HOME)/.local/share/kraftkit/sources/apps/elfloader/*.tar.gz
