# Build BusyBox from source for Hyperlight/Unikraft
#
# Features:
# - Statically linked (no shared library dependencies)
# - All applets run in-process without fork() via NOFORK patch
# - Full set of coreutils available (ash shell, ls, cat, grep, sed, etc.)

FROM alpine:3 AS builder

RUN apk add --no-cache build-base linux-headers perl

ARG BUSYBOX_VERSION=1.36.1
RUN wget -q https://busybox.net/downloads/busybox-${BUSYBOX_VERSION}.tar.bz2 && \
    tar xf busybox-${BUSYBOX_VERSION}.tar.bz2

WORKDIR /busybox-${BUSYBOX_VERSION}

# Start with default config (includes most useful applets)
RUN make defconfig

# Configure for unikernel use:
#   - Static binary (no musl/libc shared library needed)
#   - Prefer internal applets over PATH lookup
#   - Shell standalone mode (use internal applets directly)
#   - NOFORK support (run applets in-process)
#   - Point exec path to /bin/busybox (no /proc available)
RUN sed -i \
        -e 's/# CONFIG_STATIC is not set/CONFIG_STATIC=y/' \
        -e 's/# CONFIG_FEATURE_PREFER_APPLETS is not set/CONFIG_FEATURE_PREFER_APPLETS=y/' \
        -e 's/# CONFIG_FEATURE_SH_STANDALONE is not set/CONFIG_FEATURE_SH_STANDALONE=y/' \
        -e 's/# CONFIG_FEATURE_SH_NOFORK is not set/CONFIG_FEATURE_SH_NOFORK=y/' \
        -e 's|CONFIG_BUSYBOX_EXEC_PATH=.*|CONFIG_BUSYBOX_EXEC_PATH="/bin/busybox"|' \
        -e 's/CONFIG_TC=y/# CONFIG_TC is not set/' \
        .config && \
    yes "" | make oldconfig

# Patch: Force ALL applets to run as NOFORK (in-process, no fork()).
# BusyBox classifies applets as Regular (fork+exec), NOEXEC (fork only),
# or NOFORK (in-process). In a unikernel, fork() is either unsupported
# or very expensive. This patch makes every applet run directly in the
# shell process, like a builtin command.
RUN printf '\n/* Hyperlight: force all applets to run in-process */\n#undef APPLET_IS_NOFORK\n#define APPLET_IS_NOFORK(i) 1\n' \
        >> include/busybox.h

RUN make -j$(nproc)

# Assemble rootfs with applet symlinks
RUN mkdir -p /rootfs/bin /rootfs/tmp && \
    cp busybox /rootfs/bin/busybox && \
    for applet in $(./busybox --list); do \
        ln -s busybox /rootfs/bin/"$applet"; \
    done

FROM scratch
COPY --from=builder /rootfs/ /
