from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class PluginSdkBundleTests(unittest.TestCase):
    # The repository no longer ships CI or release workflows. Assertions about
    # workflow files were removed; only checks against scripts that still exist
    # in this checkout are kept.

    def test_linux_appimage_verification_checks_both_sdk_files(self):
        verification = (ROOT / "scripts/verify-linux-appimage.sh").read_text(encoding="utf-8")
        for filename in ["pymss-sdk.js", "pymss-sdk.css"]:
            self.assertIn(f'test -s "$RESOURCES/resources/plugin-sdk/{filename}"', verification)

    @unittest.skipUnless(os.name == "nt", "Windows PowerShell staging")
    def test_windows_sdk_staging_copies_the_actual_files(self):
        powershell = shutil.which("powershell") or shutil.which("pwsh")
        if not powershell:
            self.skipTest("PowerShell is unavailable")
        with tempfile.TemporaryDirectory() as directory:
            stage = Path(directory) / "Portable app"
            result = subprocess.run([
                powershell, "-NoProfile", "-NonInteractive", "-File", str(ROOT / "scripts/stage-plugin-sdk.ps1"),
                "-StagePath", str(stage),
            ], capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            for filename in ["pymss-sdk.js", "pymss-sdk.css"]:
                source = ROOT / "src-tauri/resources/plugin-sdk" / filename
                self.assertEqual((stage / "python/plugin-sdk" / filename).read_bytes(), source.read_bytes())


if __name__ == "__main__":
    unittest.main()
