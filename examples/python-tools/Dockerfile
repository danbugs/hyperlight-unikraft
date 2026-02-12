# Python rootfs for tool-calling example
# Based on examples/python/Dockerfile but keeps json module for tool calling

FROM ubuntu:22.04 as base

ENV PATH /usr/local/bin:$PATH

RUN set -xe ; \
	apt update ; \
	apt install -yqq build-essential ; \
	apt install -yqq make ; \
	apt install -yqq curl ; \
	apt install -yqq tar ; \
	apt install -yqq libexpat-dev ; \
	apt install -yqq zlib1g-dev

ARG PYTHON_VERSION=3.12.0
ENV PYTHON_VERSION ${PYTHON_VERSION}

ENV SRCDIR /python
RUN set -xe ; \
	mkdir -p ${SRCDIR}/build ; \
	curl -sL "https://www.python.org/ftp/python/${PYTHON_VERSION}/Python-${PYTHON_VERSION}.tar.xz" | \
	tar -xJC ${SRCDIR} --strip-components=1 -f -

WORKDIR ${SRCDIR}/build

RUN set -xe ; \
	gnuArch="$(dpkg-architecture --query DEB_BUILD_GNU_TYPE)" ;  \
	../configure \
	--build="$gnuArch" \
	--enable-shared

RUN set -xe ; \
	make -j $(( 1 * $( egrep '^processor[[:space:]]+:' /proc/cpuinfo | wc -l ) )) ; \
	make install

RUN set -xe ; \
	find /usr/local -type f -name "*.so" -exec strip --strip-unneeded {} + ; \
	find /usr/local -type f -name "*.so" | ldconfig ; \
	find /usr/local -depth \
	\( \
		\( -type d -a \( -name test -o -name tests -o -name __pycache__ \) \) \
			-o \
		\( -type f -a \( -name '*.pyc' -o -name '*.pyo' -o -name '*.exe' \) \) \
	\) -exec rm -rf '{}' +;

RUN set -xe ; \
	rm -fr /usr/local/lib/python*/config-* ; \
	rm -rf /usr/local/lib/python*/ensurepip ; \
	rm -rf /usr/local/lib/python*/site-packages/pip* ; \
	rm -rf /usr/local/lib/python*/site-packages/setuptools* ; \
	rm -rf /usr/local/lib/python*/idlelib ; \
	rm -rf /usr/local/lib/python*/tkinter ; \
	rm -rf /usr/local/lib/python*/turtledemo ; \
	rm -rf /usr/local/lib/python*/turtle.py ; \
	rm -rf /usr/local/lib/python*/pydoc.py ; \
	rm -rf /usr/local/lib/python*/pydoc_data ; \
	rm -rf /usr/local/lib/python*/doctest.py ; \
	rm -rf /usr/local/lib/python*/unittest ; \
	rm -rf /usr/local/lib/python*/lib2to3 ; \
	rm -rf /usr/local/lib/python*/distutils ; \
	rm -rf /usr/local/lib/python*/venv ; \
	rm -rf /usr/local/lib/python*/curses ; \
	rm -rf /usr/local/lib/python*/sqlite3 ; \
	rm -rf /usr/local/lib/python*/multiprocessing ; \
	rm -rf /usr/local/lib/python*/xmlrpc ; \
	rm -rf /usr/local/lib/python*/html ; \
	rm -rf /usr/local/lib/python*/http ; \
	rm -rf /usr/local/lib/python*/email ; \
	rm -rf /usr/local/lib/python*/logging ; \
	rm -rf /usr/local/lib/python*/xml ; \
	rm -rf /usr/local/lib/python*/asyncio ; \
	rm -rf /usr/local/lib/python*/concurrent ; \
	rm -rf /usr/local/lib/python*/ctypes ; \
	rm -rf /usr/local/lib/python*/dbm ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_test* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_codecs_jp* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_codecs_hk* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_codecs_cn* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_codecs_kr* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_codecs_tw* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_codecs_iso* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_decimal* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_multiprocess* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_sqlite* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_ctypes* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/pyexpat* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_asyncio* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_lzma* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/_bz2* ; \
	rm -rf /usr/local/lib/python*/lib-dynload/unicodedata* ; \
	rm -f /usr/local/lib/python*/_pydecimal.py ; \
	rm -rf /usr/local/bin/idle3* ; \
	rm -rf /usr/local/bin/2to3* ; \
	rm -rf /usr/local/bin/pip* ; \
	rm -rf /usr/local/bin/pydoc* ; \
	strip -s /usr/local/lib/libpython*.so.* ; \
	strip -s /usr/local/bin/python3.* 2>/dev/null || true

FROM scratch

COPY --from=base /usr/local/bin/ /usr/local/bin/
COPY --from=base /usr/local/lib/ /usr/local/lib/
COPY --from=base /lib/x86_64-linux-gnu/libc.so.6 /lib/x86_64-linux-gnu/libc.so.6
COPY --from=base /lib/x86_64-linux-gnu/libm.so.6 /lib/x86_64-linux-gnu/libm.so.6
COPY --from=base /lib64/ld-linux-x86-64.so.2 /lib64/ld-linux-x86-64.so.2
COPY --from=base /etc/ld.so.cache /etc/ld.so.cache

# Hyperlight guest SDK - available as `import hyperlight`
COPY hyperlight.py /usr/local/lib/python3.12/hyperlight.py
