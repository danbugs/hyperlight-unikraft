FROM alpine:3.20 AS build
RUN apk add --no-cache musl-dev gcc
COPY main.c /src/main.c
# app-elfloader requires static-pie ELFs (dynamic or static-pie only).
RUN gcc -static-pie -fPIE -fno-stack-protector -O2 \
    -o /src/multifn-c /src/main.c

FROM alpine:3.20 AS cpio
RUN apk add --no-cache cpio findutils
RUN mkdir -p /rootfs/bin
COPY --from=build /src/multifn-c /rootfs/bin/multifn-c
RUN cd /rootfs && find . | cpio -o -H newc > /output.cpio 2>/dev/null
