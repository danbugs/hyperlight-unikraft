ARG BASE=ghcr.io/danbugs/hyperlight-unikraft/python-base:latest
FROM ${BASE} AS rootfs
COPY hyperlight.py /hyperlight.py
COPY hostfs_demo.py /hostfs_demo.py

FROM alpine:3.20 AS cpio
RUN apk add --no-cache cpio findutils
COPY --from=rootfs / /rootfs/
RUN cd /rootfs && find . | cpio -o -H newc > /output.cpio 2>/dev/null
