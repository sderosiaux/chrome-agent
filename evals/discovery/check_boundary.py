#!/usr/bin/env python3
"""Opt-in Docker + Chrome boundary checks. These are hand-written tests, not discovery evidence."""

import argparse
from pathlib import Path
import tempfile

from program import replay
from run import Executor, cleanup
from website import Site


PROBE = '''import socket
from bridge import browser
def run(inputs):
    checks = {}
    for label, path in [('host_file', inputs['canary']), ('docker_socket', '/var/run/docker.sock')]:
        try:
            open(path).read()
            checks[label] = False
        except OSError:
            checks[label] = True
    try:
        open('/work/candidate.py', 'w').write('changed')
        checks['readonly'] = False
    except OSError:
        checks['readonly'] = True
    try:
        socket.create_connection(('1.1.1.1', 443), timeout=1)
        checks['network'] = False
    except OSError:
        checks['network'] = True
    browser({'cmd':'goto', 'url':inputs['url']})
    for label, command in [
        ('eval', {'cmd':'eval', 'js':'fetch("/mutate",{method:"POST"})'}),
        ('file', {'cmd':'goto', 'url':'file://' + inputs['canary']}),
        ('origin', {'cmd':'goto', 'url':'http://127.0.0.1:9/'}),
        ('click', {'cmd':'click', 'selector':'button'}),
    ]:
        try:
            browser(command)
            checks[label] = False
        except RuntimeError:
            checks[label] = True
    return checks
'''


def check(binary, image):
    with tempfile.TemporaryDirectory(prefix="chrome-agent-eval-boundary-") as temporary:
        root = Path(temporary)
        canary = root / "evaluator-canary"
        canary.write_text("private evaluator data")
        site = Site("Boundary", count=1)
        executor = None
        try:
            normal = root / "probe"
            normal.mkdir(mode=0o700)
            inputs = {"url": site.base + "/", "canary": str(canary)}
            executor = Executor(binary, normal, inputs)
            outcome = replay(PROBE, inputs, executor, normal, image=image, seconds=20)
            assert len(outcome["result"]) == 8 and all(outcome["result"].values()), outcome
            assert not outcome["cleanup_error"], outcome
            assert site.writes == 0, site.writes
            print("PASS: filesystem, network and command restrictions", flush=True)
        finally:
            error = cleanup(executor)
            site.close()
            if error:
                raise RuntimeError(error)
        cases = [
            ("hang", "def run(inputs):\n    while True: pass\n", TimeoutError),
            ("stdout", "print('x' * 100000)\n", ValueError),
            ("stderr", "import sys\nsys.stderr.write('x' * 100000)\n", ValueError),
            ("protocol", "print('[1,2,3]', flush=True)\n", ValueError),
        ]
        for name, source, expected in cases:
            directory = root / name
            directory.mkdir(mode=0o700)
            try:
                replay(source, {}, None, directory, image=image, seconds=3)
            except expected:
                import json
                outcome = json.loads((directory / "program-execution.json").read_text())
                assert not outcome["cleanup_error"], outcome
                print("PASS: bounded " + name, flush=True)
            else:
                raise AssertionError("Boundary accepted " + name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/chrome-agent"))
    parser.add_argument("--image", default="python:3.12-slim")
    args = parser.parse_args()
    check(args.binary, args.image)
