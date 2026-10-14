"""Measure Brain session directories without reading their contents; report to local Aex."""
import argparse
import json
import os
from pathlib import Path
import stat
import time
import urllib.request
import subprocess


def measure_sessions(parent, result):
    for session in parent.iterdir():
        if not session.is_dir() or session.is_symlink():
            raise ValueError("unexpected session directory")
        total = 0
        for directory, dirs, files in os.walk(session, followlinks=False):
            for name in dirs + files:
                info = (Path(directory) / name).lstat()
                if stat.S_ISLNK(info.st_mode):
                    raise ValueError("symlink in session data")
                if stat.S_ISREG(info.st_mode):
                    total += info.st_size
        result[session.name] = result.get(session.name, 0) + total


def measure(root):
    if (root / "format").read_text().strip() != "brain-data/3":
        raise ValueError("unsupported Brain data format")
    result = {}
    measure_sessions(root / "sessions", result)
    for agentloop in (root / "agentloops").iterdir():
        if not agentloop.is_dir() or agentloop.is_symlink():
            raise ValueError("unexpected agentloop directory")
        measure_sessions(agentloop, result)
    return result


def send(origin, operation, container=None):
    if container:
        return json.loads(subprocess.check_output(["docker", "exec", container, "aex-server", "operate", "--request", json.dumps(operation)], timeout=120))
    from urllib.parse import urlsplit
    import ipaddress
    url = urlsplit(origin)
    if url.scheme != "http" or not ipaddress.ip_address(url.hostname).is_loopback:
        raise ValueError("operator origin must be loopback HTTP")
    request = urllib.request.Request(origin.rstrip("/") + "/operate",
        data=json.dumps(operation).encode(), headers={"content-type": "application/json",
        "authorization": "Bearer " + os.environ["AEX_OPERATOR_TOKEN"]})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--brain-data", type=Path, required=True)
    parser.add_argument("--operator-url", default="http://127.0.0.1:8082")
    parser.add_argument("--once", action="store_true")
    parser.add_argument("--operator-container")
    args = parser.parse_args()
    while True:
        observed_at = int(time.time())
        send(args.operator_url, {"action": "report_usage", "observed_at": observed_at, "sessions": measure(args.brain_data)}, args.operator_container)
        send(args.operator_url, {"action": "maintain"}, args.operator_container)
        if args.once:
            return
        time.sleep(10)


if __name__ == "__main__":
    main()
