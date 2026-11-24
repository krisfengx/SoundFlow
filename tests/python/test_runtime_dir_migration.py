"""Runtime environment directory migration (Settings -> Data directories)."""
from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import worker_bootstrap

DEBUG_TMP = Path(__file__).resolve().parents[2] / "_debug_output" / "tmp"


class RuntimeDirMigrationTests(unittest.TestCase):
    def setUp(self):
        DEBUG_TMP.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=DEBUG_TMP)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / "runtime-envs"
        self.target = self.root / "moved" / "runtime-envs"
        self.source.mkdir(parents=True)

        self.events = []
        patches = [
            mock.patch.object(worker_bootstrap, "RUNTIME_ENVS_DIR", self.source),
            mock.patch.object(worker_bootstrap, "ACTIVE_RUNTIME_FILE", self.source / "active-runtime.json"),
            mock.patch.object(worker_bootstrap, "_emit", lambda kind, payload, task_id=None: self.events.append((kind, payload))),
        ]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)

    def _write_state(self, python_path: Path):
        state = {
            "backend": "cpu",
            "pythonPath": str(python_path),
            "logPath": str(self.source / "cpu" / "pymss-runtime-install.log"),
        }
        (self.source / "active-runtime.json").write_text(json.dumps(state), encoding="utf-8")

    def test_moves_environment_and_rewrites_absolute_paths(self):
        env = self.source / "cpu"
        scripts = env / ("Scripts" if os.name == "nt" else "bin")
        scripts.mkdir(parents=True)
        python = scripts / ("python.exe" if os.name == "nt" else "python")
        python.write_text("interpreter", encoding="utf-8")
        (env / "pymss-runtime-state.json").write_text(
            json.dumps({"backend": "cpu", "pythonPath": str(python)}), encoding="utf-8"
        )
        self._write_state(python)
        if os.name == "nt":
            launcher = scripts / "pip.exe"
            launcher.write_bytes(b"#!" + os.fsencode(str(python)) + b"\r\nMZ")

        code = worker_bootstrap.cmd_migrate_runtime_envs({"targetDir": str(self.target)})

        self.assertEqual(code, 0)
        self.assertFalse((self.source / "cpu").exists())
        moved_python = self.target / "cpu" / ("Scripts" if os.name == "nt" else "bin") / python.name
        self.assertTrue(moved_python.is_file())
        active = json.loads((self.target / "active-runtime.json").read_text(encoding="utf-8"))
        self.assertEqual(active["pythonPath"], str(moved_python))
        self.assertIn(str(self.target), active["logPath"])
        state = json.loads((self.target / "cpu" / "pymss-runtime-state.json").read_text(encoding="utf-8"))
        self.assertEqual(state["pythonPath"], str(moved_python))
        if os.name == "nt":
            self.assertIn(os.fsencode(str(moved_python)), (self.target / "cpu" / "Scripts" / "pip.exe").read_bytes())
        self.assertTrue(any(kind == "runtime_dir_migration_finished" for kind, _ in self.events))

    def test_refuses_non_empty_target_without_touching_source(self):
        env = self.source / "cpu"
        env.mkdir()
        marker = env / "keep.txt"
        marker.write_text("keep", encoding="utf-8")
        self.target.mkdir(parents=True)
        (self.target / "occupied.txt").write_text("occupied", encoding="utf-8")

        code = worker_bootstrap.cmd_migrate_runtime_envs({"targetDir": str(self.target)})

        self.assertNotEqual(code, 0)
        self.assertTrue(marker.is_file())
        self.assertTrue((self.target / "occupied.txt").is_file())

    def test_empty_source_creates_target(self):
        code = worker_bootstrap.cmd_migrate_runtime_envs({"targetDir": str(self.target)})
        self.assertEqual(code, 0)
        self.assertTrue(self.target.is_dir())
        finished = [payload for kind, payload in self.events if kind == "runtime_dir_migration_finished"]
        self.assertEqual(finished[-1]["backends"], [])


if __name__ == "__main__":
    unittest.main()
