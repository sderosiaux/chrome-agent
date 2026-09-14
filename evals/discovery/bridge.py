"""The only browser bridge mounted inside a candidate's container."""

import json
import sys


def browser(command):
    print(json.dumps({"type": "command", "command": command}), flush=True)
    line = sys.stdin.readline(65537)
    if not line or len(line) > 65536:
        raise RuntimeError("Browser bridge response missing or oversized")
    result = json.loads(line)
    if result.get("ok") is not True:
        raise RuntimeError(json.dumps(result))
    return result
