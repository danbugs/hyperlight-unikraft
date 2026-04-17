ARG BASE=ghcr.io/danbugs/hyperlight-unikraft/python-base:latest
FROM ${BASE} AS rootfs
COPY test_tools.py /test_tools.py

FROM alpine:3.20 AS cpio
RUN apk add --no-cache cpio findutils
COPY --from=rootfs / /rootfs/
RUN cd /rootfs && find . | cpio -o -H newc > /output.cpio 2>/dev/null
