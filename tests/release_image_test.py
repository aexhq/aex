import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("release_image", Path(__file__).parents[1] / "tools/release-image.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseImageTest(unittest.TestCase):
    def test_missing_is_distinct_from_registry_failure(self):
        for error in ["ERROR: image: not found", "manifest unknown"]:
            with patch.object(release.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", error)):
                self.assertEqual(release.resolve("ghcr.io/aexhq/test:sha-x"), "")
        for error in ["unauthorized", "timeout", "certificate invalid", "command not found"]:
            with patch.object(release.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", error)):
                with self.assertRaises(RuntimeError):
                    release.resolve("ghcr.io/aexhq/test:sha-x")

    def test_existing_image_resolves_to_original_digest(self):
        with patch.object(release, "inspect", return_value={"digest": "sha256:original"}):
            self.assertEqual(release.resolve("ghcr.io/aexhq/test:sha-x"), "ghcr.io/aexhq/test@sha256:original")

    def test_rerun_reuses_manifest_and_collision_never_overwrites(self):
        entries = [{"digest": f"sha256:{arch}", "platform": {"architecture": arch, "os": "linux"}} for arch in ["amd64", "arm64"]]
        children = [f"ghcr.io/aexhq/test@sha256:{arch}" for arch in ["amd64", "arm64"]]
        with patch.object(release, "inspect", return_value={"digest": "sha256:original"}), patch.object(release.subprocess, "run") as run:
            with patch.object(release.subprocess, "check_output", return_value=json.dumps({"manifests": entries})):
                self.assertTrue(release.manifest("ghcr.io/aexhq/test:sha-x", children).endswith("@sha256:original"))
            entries[0]["digest"] = "sha256:unexpected"
            with patch.object(release.subprocess, "check_output", return_value=json.dumps({"manifests": entries})):
                with self.assertRaisesRegex(RuntimeError, "refusing to overwrite"):
                    release.manifest("ghcr.io/aexhq/test:sha-x", children)
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
