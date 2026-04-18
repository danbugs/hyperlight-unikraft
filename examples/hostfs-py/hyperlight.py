"""Hyperlight guest SDK — call host tools from inside the VM, including
the host-mediated filesystem sandbox (fs_read / fs_write / fs_list /
fs_stat / fs_mkdir / fs_unlink). All FS calls are scoped to the host
directory passed to `hyperlight-unikraft --mount`."""
import json


def call_tool(name, **kwargs):
    """Call a host-registered tool by name.

    Args:
        name: Tool name (must be registered on the host).
        **kwargs: Tool arguments (JSON-serializable).

    Returns:
        The tool's result (deserialized from JSON).

    Raises:
        RuntimeError: If the host returns an error.
    """
    request = json.dumps({"name": name, "args": kwargs})
    fd = open("/dev/hcall", "r+b", buffering=0)
    fd.write(request.encode())
    result = json.loads(fd.read())
    fd.close()
    if "error" in result:
        raise RuntimeError(result["error"])
    return result.get("result")


# ---------------------------------------------------------------------------
# Filesystem sandbox — thin wrappers over the fs_* tools.
# ---------------------------------------------------------------------------

def fs_read(path):
    """Read text from a file under the mount root."""
    return call_tool("fs_read", path=path)["text"]


def fs_write(path, text, append=False):
    """Write text to a file under the mount root."""
    return call_tool("fs_write", path=path, text=text, append=append)["bytes_written"]


def fs_list(path=""):
    """List a directory under the mount root. Returns a list of entries,
    each a dict with keys: name, is_dir, is_file, is_symlink."""
    return call_tool("fs_list", path=path)["entries"]


def fs_stat(path):
    """Return {'size': int, 'is_dir': bool, 'is_file': bool} for a path."""
    return call_tool("fs_stat", path=path)


def fs_mkdir(path, parents=False):
    """Create a directory under the mount root."""
    call_tool("fs_mkdir", path=path, parents=parents)


def fs_unlink(path):
    """Remove a file or empty directory under the mount root."""
    call_tool("fs_unlink", path=path)
