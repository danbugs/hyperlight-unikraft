"""Hyperlight guest SDK - call host tools from inside the VM."""
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
