# A "productivity agent" Python environment: the trimmed python-base
# + a pure-Python dependency stack covering config, templating, data
# tables, HTML/Markdown, PDF, Excel, CLI wiring, and retry/progress
# primitives. Everything in here is pip-installable pure Python — no
# C extensions — so it drops straight onto the existing python-base
# runtime with no ABI surprises.

# BASE is resolved pre-FROM so the classic (non-BuildKit) docker
# frontend used by DOCKER_BUILDKIT=0 handles the substitution.
ARG BASE=ghcr.io/danbugs/hyperlight-unikraft/python-base:latest

# Stage 1: resolve + install pip deps into a target dir.
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
    python-dateutil

# No subprocess-ready binaries yet — the kernel's vfork+execve works,
# but app-elfloader crashes the child on stack setup (page fault at
# low addresses during any exec, even Python → Python). Tracked as
# follow-up work. For now the agent runs as a single process.

# Stage 2: layer deps onto python-base.
FROM ${BASE} AS rootfs
COPY --from=deps /deps /usr/local/lib/python3.12/site-packages
COPY agent.py /agent.py

# Stage 3: pack as CPIO.
FROM alpine:3.20 AS cpio
RUN apk add --no-cache cpio findutils
COPY --from=rootfs / /rootfs/
RUN cd /rootfs && find . | cpio -o -H newc > /output.cpio 2>/dev/null
