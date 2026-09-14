"""Run an agent-authored Python candidate with no network or host filesystem access."""

import ast
import hashlib
import json
import os
from pathlib import Path
import secrets
import select
import selectors
import shutil
import subprocess
import time


RUNNER = """import json,sys
inputs=json.loads(sys.stdin.readline(65537))
import candidate
print(json.dumps({"type":"result","result":candidate.run(inputs)}),flush=True)
"""


def send(child, value, deadline):
    data = (json.dumps(value) + "\n").encode()
    if len(data) > 65536:
        raise ValueError("Bridge message exceeded 64 KiB")
    fd = child.stdin.fileno()
    while data:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([], [fd], [], remaining)[1]:
            raise TimeoutError("Candidate stopped reading within its runtime budget")
        try:
            data = data[os.write(fd, data[:4096]):]
        except BlockingIOError:
            continue


def replay(source, inputs, executor, directory, *, image="python:3.12-slim", seconds=60):
    if not isinstance(source, str) or len(source.encode()) > 65536:
        raise ValueError("Candidate source must be at most 64 KiB")
    ast.parse(source)  # Syntax only. Never import or execute candidate code in this process.
    stage = directory / "program"
    stage.mkdir(mode=0o755)
    (stage / "candidate.py").write_text(source)
    (stage / "runner.py").write_text(RUNNER)
    shutil.copyfile(Path(__file__).with_name("bridge.py"), stage / "bridge.py")
    for path in stage.iterdir():
        path.chmod(0o444)
    name = "chrome-agent-eval-" + secrets.token_hex(8)
    inspect = subprocess.run(["docker", "image", "inspect", image, "--format", "{{.Id}}"],
                             capture_output=True, text=True, timeout=10, check=True)
    image_id = inspect.stdout.strip()
    (directory / "program-manifest.json").write_text(json.dumps({
        "image_id": image_id, "candidate_sha256": hashlib.sha256(source.encode()).hexdigest(),
        "seconds": seconds, "max_commands": 60,
    }, indent=2) + "\n")
    args = ["docker", "run", "--name", name, "-i", "--network", "none", "--read-only",
            "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--pids-limit", "32",
            "--memory", "128m", "--cpus", "1", "--user", "65534:65534",
            "--mount", f"type=bind,src={stage.resolve()},dst=/work,readonly",
            "--tmpfs", "/tmp:rw,noexec,nosuid,size=16777216", "--workdir", "/work",
            image_id, "python", "-B", "/work/runner.py"]
    stderr = (directory / "program-stderr.log").open("wb")
    child = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    os.set_blocking(child.stdin.fileno(), False)
    poll = selectors.DefaultSelector()
    poll.register(child.stdout, selectors.EVENT_READ, "stdout")
    poll.register(child.stderr, selectors.EVENT_READ, "stderr")
    deadline = time.monotonic() + seconds
    buffer = b""
    result = None
    commands = 0
    stderr_bytes = 0
    outcome = {"result": None, "commands": 0, "image_id": image_id,
               "candidate_sha256": hashlib.sha256(source.encode()).hexdigest(), "cleanup_error": None}
    try:
        send(child, inputs, deadline)
        while result is None:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not (ready := poll.select(remaining)):
                raise TimeoutError("Candidate runtime budget exhausted")
            eof = False
            for key, _ in ready:
                chunk = os.read(key.fileobj.fileno(), 4096)
                if not chunk:
                    poll.unregister(key.fileobj)
                    eof = eof or key.data == "stdout"
                elif key.data == "stderr":
                    stderr_bytes += len(chunk)
                    if stderr_bytes > 65536:
                        raise ValueError("Candidate stderr exceeded 64 KiB")
                    stderr.write(chunk)
                    stderr.flush()
                else:
                    buffer += chunk
            if len(buffer) > 65536:
                raise ValueError("Candidate protocol line exceeded 64 KiB")
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                message = json.loads(line)
                if not isinstance(message, dict):
                    raise ValueError("Candidate protocol requires JSON objects")
                if message.get("type") == "result" and set(message) == {"type", "result"}:
                    result = message["result"]
                    if not isinstance(result, dict):
                        raise ValueError("Candidate result must be an object")
                    break
                if message.get("type") != "command" or set(message) != {"type", "command"}:
                    raise ValueError("Candidate protocol accepts only commands and a final result")
                commands += 1
                if commands > 60:
                    raise ValueError("Candidate command budget exhausted")
                state = json.loads(executor.file.read_text())
                proposal = {"id": f"program-{commands}", "revision": state["revision"],
                            "reason": "Execute the frozen candidate procedure", "command": message["command"]}
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("Candidate runtime budget exhausted")
                receipt = executor.step(proposal, timeout=min(40, remaining))
                response = receipt
                if receipt.get("ok") and receipt.get("experiment", {}).get("results"):
                    response = receipt["experiment"]["results"][0]["response"]
                send(child, response, deadline)
            if eof and result is None:
                raise ValueError("Candidate exited without a result; see program-stderr.log")
        outcome["result"] = result
        return outcome
    finally:
        poll.close()
        if child.poll() is None:
            child.kill()
        child.wait(timeout=5)
        try:
            removed = subprocess.run(["docker", "rm", "-f", name], capture_output=True,
                                     text=True, timeout=10, check=False)
            if removed.returncode and "No such container" not in removed.stderr:
                raise ValueError("Could not remove owned candidate container: " + removed.stderr)
        except (OSError, ValueError, subprocess.SubprocessError) as exc:
            outcome["cleanup_error"] = str(exc)
        finally:
            child.stdin.close()
            child.stdout.close()
            child.stderr.close()
            stderr.close()
            outcome["commands"] = commands
            # Preserve a submitted result even when cleanup fails; never call such a run accepted.
            (directory / "program-execution.json").write_text(json.dumps(outcome, indent=2) + "\n")
