from __future__ import annotations

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


class PackagedLauncherTests(unittest.TestCase):
    def test_installed_layout_ignores_other_path_shims_and_forwards_arguments(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            package = root / "site-packages" / "neuron_cli"
            package.mkdir(parents=True)
            shutil.copyfile(
                Path(__file__).resolve().parents[1] / "neuron_cli" / "__init__.py",
                package / "__init__.py",
            )
            spec = importlib.util.spec_from_file_location("installed_neuron", package / "__init__.py")
            launcher = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(launcher)

            stale = root / "old-scripts"
            stale.mkdir()
            (stale / "neuron").write_text("old shim", encoding="utf-8")
            (stale / "neuron.exe").write_text("old shim", encoding="utf-8")
            binary = root / "downloaded-version-matched-neuron"
            with patch.dict(os.environ, {"PATH": str(stale), "NEURON_BINARY_PATH": ""}), patch.object(
                launcher, "_download_binary", return_value=binary
            ) as download, patch.object(sys, "argv", [str(root / "scripts" / "neuron"), "prompt", "two words"]), patch.object(
                launcher.subprocess, "run", return_value=subprocess.CompletedProcess([], 7)
            ) as run:
                with self.assertRaises(SystemExit) as result:
                    launcher.main()
                self.assertEqual(result.exception.code, 7)
                download.assert_called_once_with()
                run.assert_called_once_with([str(binary), "prompt", "two words"], check=False)

            native = root / "native-neuron"
            native.touch()
            with patch.dict(os.environ, {"NEURON_BINARY_PATH": str(native)}):
                self.assertEqual(launcher._find_binary(), native.resolve())
            with patch.dict(os.environ, {"NEURON_BINARY_PATH": str(root / "missing")}):
                with self.assertRaises(RuntimeError):
                    launcher._find_binary()


if __name__ == "__main__":
    unittest.main()
