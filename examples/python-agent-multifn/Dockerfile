# python-agent-multifn — python-agent reshaped for Hyperlight's native
# multi-function dispatch. A small C driver (hl_pydriver) embeds Python
# 3.12 via libpython3.12.so and exposes:
#   init()     - Py_Initialize + heavy imports
#   run(code)  - PyRun_SimpleString(code)
#
# The host calls init() once after evolve, snapshots the post-warm-up
# state, and on every subsequent call restores + calls run(<user code>).
# numpy / pandas / pydantic / ... stay loaded in sys.modules across
# calls for free.

ARG BASE=ghcr.io/danbugs/hyperlight-unikraft/python-base:latest

# Stage 1: install Python deps + trim. Same as python-agent.
FROM python:3.12-slim AS deps
RUN pip install --target=/deps --no-cache-dir \
    tqdm \
    pyyaml \
    jinja2 \
    beautifulsoup4 \
    tabulate \
    click \
    tenacity \
    python-dotenv \
    pypdf \
    openpyxl \
    markdown-it-py \
    pydantic \
    pillow \
    lxml \
    cryptography \
    python-dateutil \
    numpy \
    pandas
RUN set -eux; \
    find /deps -maxdepth 1 -type d -name '*.dist-info' -exec rm -rf {} + 2>/dev/null || true; \
    find /deps \( -type d -name tests -o -type d -name test \) -exec rm -rf {} + 2>/dev/null || true; \
    find /deps -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true; \
    find /deps -type f -name '*.pyc' -delete 2>/dev/null || true; \
    find /deps -name '*.so*' -exec strip --strip-unneeded {} + 2>/dev/null || true

# Stage 2: build hl_pydriver. IMPORTANT — this stage is the SAME
# ubuntu:22.04 + Python-3.12-from-source that produces local-python-base,
# so the glibc version the driver is linked against matches what the
# guest has at runtime. The runtimes/python.Dockerfile `base` stage is
# tagged as `local-python-base-dev` by the project's Justfile.
FROM local-python-base-dev AS driver-build
COPY hl_pydriver.c /src/hl_pydriver.c
# Python 3.12 from source lands headers at /usr/local/include/python3.12
# and libpython3.12.so.1.0 at /usr/local/lib.
RUN PY_INC=$(python3.12 -c 'import sysconfig; print(sysconfig.get_path("include"))') && \
    PY_LIBDIR=$(python3.12 -c 'import sysconfig; print(sysconfig.get_config_var("LIBDIR"))') && \
    gcc -O2 -fPIE -pie -Wl,-E \
        -I$PY_INC \
        -o /src/hl_pydriver /src/hl_pydriver.c \
        -L$PY_LIBDIR \
        -Wl,-rpath,/usr/local/lib \
        -Wl,-rpath,/lib/x86_64-linux-gnu \
        -lpython3.12 -ldl -lpthread -lm -lutil

# Stage 3: layer deps + driver onto python-base.
FROM ${BASE} AS rootfs
COPY --from=deps /deps /usr/local/lib/python3.12/site-packages
COPY --from=driver-build /src/hl_pydriver /bin/hl_pydriver

# Stage 4: pack as CPIO.
FROM alpine:3.20 AS cpio
RUN apk add --no-cache cpio findutils
COPY --from=rootfs / /rootfs/
RUN cd /rootfs && find . | cpio -o -H newc > /output.cpio 2>/dev/null
