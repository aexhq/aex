"""Resolve existing release images; never treat a registry outage as an absent tag."""
import json
import re
import subprocess
import sys


def inspect(image):
    result = subprocess.run(
        ["docker", "buildx", "imagetools", "inspect", image, "--format", "{{json .Manifest}}"],
        capture_output=True, text=True, check=False,
    )
    if result.returncode:
        if re.search(r"(?:manifest unknown|: not found)\s*$", result.stderr.strip(), re.I):
            return None
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout)


def resolve(image):
    manifest = inspect(image)
    return "" if manifest is None else image.rsplit(":", 1)[0] + "@" + manifest["digest"]


def manifest(image, children):
    expected = dict(zip(("amd64", "arm64"), (child.split("@", 1)[1] for child in children)))
    if inspect(image) is None:
        subprocess.run(["docker", "buildx", "imagetools", "create", "--tag", image, *children], check=True)
    document = json.loads(subprocess.check_output(
        ["docker", "buildx", "imagetools", "inspect", image, "--raw"], text=True,
    ))
    entries = document.get("manifests", [])
    actual = {item["platform"]["architecture"]: item["digest"] for item in entries}
    if len(entries) != 2 or actual != expected or any(item["platform"]["os"] != "linux" for item in entries):
        raise RuntimeError(f"{image} has different children; refusing to overwrite the release")
    return resolve(image)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "resolve":
        print(resolve(sys.argv[2]))
    elif len(sys.argv) == 5 and sys.argv[1] == "manifest":
        print(manifest(sys.argv[2], sys.argv[3:]))
    else:
        raise SystemExit("usage: release-image.py resolve IMAGE | manifest IMAGE AMD64 ARM64")
