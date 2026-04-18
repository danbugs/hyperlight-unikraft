#!/usr/bin/env python3
"""Exercise the host-mediated filesystem sandbox from Python.

The host is started with `--mount <dir>`; every path we pass here is
resolved relative to that directory. Escape attempts (`..`, absolute
paths that jump out, symlinks pointing outside) are rejected by the host.
"""
from hyperlight import fs_read, fs_write, fs_list, fs_stat, fs_mkdir

print("hostfs-py: exercising the host filesystem sandbox")

# 1. Write a file.
greeting = "Hello from the Unikraft guest (Python)!\nSecond line.\n"
n = fs_write("greeting.txt", greeting)
print(f"wrote greeting.txt ({n} bytes)")

# 2. Read it back.
got = fs_read("greeting.txt")
print(f"read greeting.txt ({len(got)} bytes):\n---\n{got}---")

# 3. Create a subdirectory, write into it.
fs_mkdir("logs", parents=True)
fs_write("logs/app.log", "line 1\n")
fs_write("logs/app.log", "line 2\n", append=True)
print(f"logs/app.log:\n---\n{fs_read('logs/app.log')}---")

# 4. Stat + list.
meta = fs_stat("greeting.txt")
print(f"stat greeting.txt: size={meta['size']} is_dir={meta['is_dir']}")
print("mount root contents:")
for entry in fs_list(""):
    kind = "d" if entry["is_dir"] else "f"
    print(f"  {kind} {entry['name']}")

# 5. Escape attempts must be rejected by the host.
print("escape attempts (all should raise):")
for bad in ("../etc/passwd", "subdir/../../outside.txt"):
    try:
        fs_read(bad)
        print(f"  {bad}: UNEXPECTEDLY SUCCEEDED")
    except RuntimeError as e:
        print(f"  {bad}: rejected — {e}")

print("done.")
