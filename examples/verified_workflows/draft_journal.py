"""Private, locked record of whether this operation may already have been submitted."""

import fcntl
import json
import os
from pathlib import Path
import tempfile


class Journal:
    def __init__(self, path, request):
        self.path = Path(path).expanduser().absolute()
        self.request = request
        self.data = None
        self.lock = None

    def __enter__(self):
        if not self.path.parent.is_dir():
            raise ValueError("Journal directory must already exist")
        self.lock = os.open(str(self.path) + ".lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            os.fchmod(self.lock, 0o600)
            fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            if os.path.lexists(self.path):
                descriptor = os.open(self.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                with os.fdopen(descriptor, "rb") as file:
                    import stat
                    if not stat.S_ISREG(os.fstat(file.fileno()).st_mode):
                        raise ValueError("Journal must be a regular file")
                    os.fchmod(file.fileno(), 0o600)
                    raw = file.read(65537)
                if len(raw) > 65536:
                    raise ValueError("Journal exceeds 64 KiB")
                data = json.loads(raw)
                if (not isinstance(data, dict) or type(data.get("schema")) is not int or data.get("schema") != 1
                        or data.get("request") != self.request
                        or data.get("state") not in ("prepared", "attempted", "verified")):
                    raise ValueError("Journal does not describe this exact request")
                self.data = data
            else:
                self.save("prepared")
            return self
        except BaseException:
            os.close(self.lock)
            self.lock = None
            raise

    @property
    def state(self):
        return self.data["state"]

    def save(self, state, record=None):
        data = dict(schema=1, request=self.request, state=state)
        if record is not None:
            data["record"] = record
        descriptor, temporary = tempfile.mkstemp(prefix=".draft-journal-", dir=self.path.parent)
        try:
            with os.fdopen(descriptor, "w", encoding="utf-8") as file:
                json.dump(data, file, ensure_ascii=False)
                file.flush()
                os.fsync(file.fileno())
            os.replace(temporary, self.path)
            directory = os.open(self.path.parent, os.O_RDONLY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
            self.data = data
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)

    def __exit__(self, exc_type, exc_value, traceback):
        fcntl.flock(self.lock, fcntl.LOCK_UN)
        os.close(self.lock)
