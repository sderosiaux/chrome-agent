"""Bound subprocess output and elapsed time while draining both output streams."""

import os
import selectors
import subprocess
import tempfile
import time


def bounded_run(args, *, input="", cwd=None, timeout=120, limit=1024 * 1024):
    with tempfile.TemporaryFile() as stdin:
        stdin.write(input.encode())
        stdin.seek(0)
        with subprocess.Popen(args, stdin=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=cwd) as child:
            streams = {"stdout": bytearray(), "stderr": bytearray()}
            deadline = time.monotonic() + timeout
            try:
                with selectors.DefaultSelector() as poll:
                    poll.register(child.stdout, selectors.EVENT_READ, "stdout")
                    poll.register(child.stderr, selectors.EVENT_READ, "stderr")
                    while poll.get_map():
                        remaining = deadline - time.monotonic()
                        if remaining <= 0 or not (ready := poll.select(remaining)):
                            raise subprocess.TimeoutExpired(args, timeout, bytes(streams["stdout"]), bytes(streams["stderr"]))
                        for key, _ in ready:
                            chunk = os.read(key.fileobj.fileno(), 4096)
                            if not chunk:
                                poll.unregister(key.fileobj)
                                continue
                            streams[key.data].extend(chunk)
                            if len(streams[key.data]) > limit:
                                raise ValueError(f"Subprocess {key.data} exceeded {limit} bytes")
                    child.wait(timeout=max(0.001, deadline - time.monotonic()))
                return subprocess.CompletedProcess(args, child.returncode,
                                                   streams["stdout"].decode(), streams["stderr"].decode())
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)
