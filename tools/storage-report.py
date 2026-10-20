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
    if not parent.exists():
        return
    for session in parent.iterdir():
        try:
            info = session.lstat()
        except FileNotFoundError:
            continue
        if not stat.S_ISDIR(info.st_mode):
            raise ValueError("unexpected session directory")
        total = 0
        for directory, dirs, files in os.walk(session, followlinks=False):
            for name in dirs + files:
                try:
                    info = (Path(directory) / name).lstat()
                except FileNotFoundError:
                    # Native SQLite sidecars can disappear between listing and measurement.
                    continue
                relative = (Path(directory) / name).relative_to(session)
                native = relative.parts[0] == "native" or relative.parts[0].startswith("native-interrupted-")
                if stat.S_ISLNK(info.st_mode) and not native:
                    raise ValueError("symlink in session data")
                if stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
                    total += info.st_size
        result[session.name] = result.get(session.name, 0) + total


def measure(root):
    if not (root / "metadata.sqlite").is_file():
        raise ValueError("Brain SQLite metadata is required before reporting usage")
    result = {}
    measure_sessions(root / "sessions", result)
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
