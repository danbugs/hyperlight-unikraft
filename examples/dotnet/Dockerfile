ARG BASE=ghcr.io/danbugs/hyperlight-unikraft/dotnet-base:latest
FROM ${BASE} AS rootfs

FROM alpine:3.20 AS cpio
RUN apk add --no-cache cpio findutils
COPY --from=rootfs / /rootfs/
RUN cd /rootfs && find . | cpio -o -H newc > /output.cpio 2>/dev/null
