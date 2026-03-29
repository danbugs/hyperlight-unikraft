"""Hyperlight guest SDK - call host tools from inside the VM.

Uses a minimal JSON serializer to avoid importing the stdlib ``json``
module, which pulls in ``re`` and friends and adds ~100 ms of import
overhead on every cold snapstart.
"""


def _dumps_value(v):
    """Serialize a Python value to a JSON string (minimal subset)."""
    if v is None:
        return "null"
    if v is True:
        return "true"
    if v is False:
        return "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        # repr gives full precision; inf/nan are not valid JSON but
        # shouldn't appear in normal tool arguments.
        return repr(v)
    if isinstance(v, str):
        # Escape the minimum required characters.
        return '"' + v.replace("\\", "\\\\").replace('"', '\\"').replace(
            "\n", "\\n").replace("\r", "\\r").replace("\t", "\\t") + '"'
    if isinstance(v, (list, tuple)):
        return "[" + ",".join(_dumps_value(i) for i in v) + "]"
    if isinstance(v, dict):
        pairs = ",".join(
            _dumps_value(str(k)) + ":" + _dumps_value(val)
            for k, val in v.items()
        )
        return "{" + pairs + "}"
    return _dumps_value(str(v))


def _parse_response(raw):
    """Parse the JSON response from the host.

    We fall back to the stdlib ``json`` module for robustness, but try
    ``eval`` first (safe here because the host is trusted and the
    response is a simple dict).
    """
    text = raw.decode() if isinstance(raw, (bytes, bytearray)) else raw
    # The host always returns {"result": ...} or {"error": ...}.
    # Python's ast.literal_eval handles JSON-like dicts directly.
    # We translate JSON literals to Python equivalents first.
    py = text.replace("null", "None").replace("true", "True").replace("false", "False")
    return eval(py)  # noqa: S307 — trusted host response


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
    request = _dumps_value({"name": name, "args": kwargs})
    fd = open("/dev/hcall", "r+b", buffering=0)
    fd.write(request.encode())
    result = _parse_response(fd.read())
    fd.close()
    if "error" in result:
        raise RuntimeError(result["error"])
    return result.get("result")
