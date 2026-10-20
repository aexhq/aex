import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("meter", Path(__file__).parent.parent / "tools/storage-report.py")
meter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(meter)


class MeterTests(unittest.TestCase):
    def test_counts_session_state_without_charging_shared_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "metadata.sqlite").write_bytes(b"shared metadata")
            (root / "sessions/ses_test").mkdir(parents=True)
            (root / "sessions/ses_test/events.jsonl").write_bytes(b"state")
            (root / "sessions/ses_test/native").mkdir(parents=True)
            (root / "sessions/ses_test/native/native.json").write_bytes(b"native")
            (root / "sessions/ses_other/native").mkdir(parents=True)
            (root / "sessions/ses_other/native/native.json").write_bytes(b"other")
            (root / "server-metadata").write_bytes(b"secret")
            self.assertEqual(meter.measure(root), {"ses_test": 11, "ses_other": 5})

    def test_unknown_format_is_an_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "format").write_text("unknown")
            with self.assertRaises(ValueError):
                meter.measure(root)

    def test_native_links_count_the_link_without_following_its_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "metadata.sqlite").write_bytes(b"shared metadata")
            (root / "sessions").mkdir()
            native = root / "sessions/ses_test/native"
            native.mkdir(parents=True)
            outside = root / "outside"
            outside.mkdir()
            (outside / "private").write_bytes(b"not session data")
            (native / "file").symlink_to(outside / "private")
            (native / "directory").symlink_to(outside, target_is_directory=True)
            (native / "dangling").symlink_to(outside / "missing")
            self.assertEqual(meter.measure(root), {"ses_test": sum(path.lstat().st_size for path in native.iterdir())})

    def test_journal_symlinks_remain_invalid(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "ses_test").mkdir()
            (root / "ses_test/link").symlink_to(root / "missing")
            with self.assertRaisesRegex(ValueError, "symlink in session data"):
                meter.measure_sessions(root, {})

    def test_native_sidecar_removed_during_scan_is_not_retained(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            session = root / "ses_test"
            session.mkdir()
            (session / "opencode.db").write_bytes(b"database")
            sidecar = session / "opencode.db-shm"
            sidecar.write_bytes(b"transient")
            walk = meter.os.walk

            def removing_walk(*args, **kwargs):
                for entry in walk(*args, **kwargs):
                    sidecar.unlink()
                    yield entry

            result = {}
            with patch.object(meter.os, "walk", removing_walk):
                meter.measure_sessions(root, result)
            self.assertEqual(result, {"ses_test": 8})
