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
            (root / "format").write_text("brain-data/3\n")
            (root / "sessions/ses_test").mkdir(parents=True)
            (root / "sessions/ses_test/journal").write_bytes(b"state")
            (root / "agentloops/pi/ses_test").mkdir(parents=True)
            (root / "agentloops/pi/ses_test/native.json").write_bytes(b"native")
            (root / "agentloops/pi/identity.json").write_bytes(b"shared runtime identity")
            (root / "agentloops/codex/ses_other").mkdir(parents=True)
            (root / "agentloops/codex/ses_other/native.json").write_bytes(b"other")
            (root / "server-metadata").write_bytes(b"secret")
            self.assertEqual(meter.measure(root), {"ses_test": 11, "ses_other": 5})

    def test_unknown_format_is_an_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "format").write_text("unknown")
            with self.assertRaises(ValueError):
                meter.measure(root)

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
