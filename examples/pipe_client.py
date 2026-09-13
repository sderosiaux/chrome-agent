"""Sequential JSONL transport shared by the runnable examples; standard library only."""

import json
import queue
import subprocess
import tempfile
import threading


class PipeError(Exception):
    """The command's response or the pipe's finalization could not be established."""


class Pipe:
    def __init__(self, binary, browser, timeout):
        self.deadline = max(10, timeout + 5)
        self.history = []
        self.messages = queue.Queue()
        self.stderr = tempfile.TemporaryFile()
        try:
            self.process = subprocess.Popen(
                [binary, "--browser", browser, "--timeout", str(timeout), "pipe"],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr,
            )
        except OSError as error:
            self.stderr.close()
            raise PipeError(f"Cannot start chrome-agent: {error}") from error
        self.reader = threading.Thread(target=self._read_stdout, daemon=True)
        self.reader.start()

    def _read_stdout(self):
        try:
            while True:
                line = self.process.stdout.readline(1024 * 1024 + 1)
                if not line:
                    self.messages.put(None)
                    return
                if len(line) > 1024 * 1024:
                    raise ValueError("pipe response exceeds this example's 1 MiB limit")
                value = json.loads(line)
                if not isinstance(value, dict) or not isinstance(value.get("ok"), bool):
                    raise ValueError("pipe response needs a boolean ok field")
                self.messages.put(value)
        except (ValueError, OSError) as error:
            self.messages.put(PipeError(f"Cannot read a pipe response: {error}"))

    def _next(self):
        try:
            message = self.messages.get(timeout=self.deadline)
        except queue.Empty as error:
            raise PipeError("Timed out waiting for a pipe response") from error
        if isinstance(message, Exception):
            raise message
        return message

    def request(self, command):
        entry = {"command": command, "response": None}
        self.history.append(entry)
        try:
            self.process.stdin.write((json.dumps(command) + "\n").encode())
            self.process.stdin.flush()
        except (OSError, ValueError) as error:
            raise PipeError("Pipe closed while sending a command") from error
        response = self._next()
        entry["response"] = response
        if response is None:
            raise PipeError("Pipe ended before the command's response")
        if response.get("terminal"):
            raise PipeError(response.get("error", "Pipe reported a terminal failure"))
        return response

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        try:
            self.process.stdin.close()
            if exc_type is None:
                # EOF and exit status matter: final persistence errors are terminal JSONL
                # messages, not responses to the last command.
                while True:
                    message = self._next()
                    if message is None:
                        break
                    if message.get("terminal"):
                        raise PipeError(message.get("error", "Pipe finalization failed"))
                    raise PipeError("Unexpected response after the last command")
                if self.process.wait(timeout=self.deadline) != 0:
                    raise PipeError("Pipe exited unsuccessfully")
        except (OSError, subprocess.TimeoutExpired) as error:
            if exc_type is None:
                raise PipeError("Pipe did not finish cleanly") from error
        finally:
            if self.process.poll() is None:
                try:
                    self.process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait()
            self.reader.join(timeout=2)
            self.process.stdout.close()
            self.stderr.close()
