FROM golang:1.21.3-bookworm AS build

WORKDIR /src

COPY ./hello.go /src/hello.go

RUN set -xe; \
    CGO_ENABLED=1 \
    go build \
      -buildmode=pie \
      -ldflags "-linkmode external -extldflags '-static-pie'" \
      -o /hello hello.go \
    ;

FROM scratch

COPY --from=build /hello /bin/hello
COPY --from=build /lib/x86_64-linux-gnu/libc.so.6 /lib/x86_64-linux-gnu/
COPY --from=build /lib64/ld-linux-x86-64.so.2 /lib64/