"""Packs npm/neuroncli, installs the tarball globally and runs it against a fake release server.

The install pulls the package's npm dependencies from the registry, so this test
runs only when NEURON_TEST_NPM_PACK=1 (packaging CI sets it):

    NEURON_TEST_NPM_PACK=1 python -m unittest tests.test_npm_package -v
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Dict, List

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fake_release  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parents[1]
PACKAGE_DIR = REPO_ROOT / "npm" / "neuroncli"
PACKAGE_NAME = "@zero-x.live/neuron"
VERSION = json.loads((PACKAGE_DIR / "package.json").read_text(encoding="utf-8"))["version"]
NPM = shutil.which("npm")


def enabled() -> bool:
    return os.environ.get("NEURON_TEST_NPM_PACK") == "1" and NPM is not None and fake_release.can_build_dummy()


def npm(*args: str, cwd: Path, env: Dict[str, str]) -> subprocess.CompletedProcess:
    assert NPM is not None
    return subprocess.run(
        [NPM, *args, "--no-audit", "--no-fund"],
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=600,
        check=False,
    )


@unittest.skipUnless(enabled(), "set NEURON_TEST_NPM_PACK=1 (needs npm and registry access)")
class NpmTarballTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls._temporary = tempfile.TemporaryDirectory()
        cls.root = Path(cls._temporary.name)
        cls.releases = cls.root / "releases"
        fake_release.stage_release(cls.releases, VERSION)
        cls.server = fake_release.FakeReleaseServer(cls.releases).start()
        packed = npm("pack", "--json", "--pack-destination", str(cls.root), cwd=PACKAGE_DIR, env=cls.base_env())
        if packed.returncode != 0:
            cls.server.stop()
            cls._temporary.cleanup()
            raise AssertionError(f"npm pack failed:\n{packed.stdout}\n{packed.stderr}")
        cls.tarball = cls.root / json.loads(packed.stdout)[0]["filename"]

    @classmethod
    def tearDownClass(cls) -> None:
        cls.server.stop()
        cls._temporary.cleanup()

    @staticmethod
    def base_env(**extra: str) -> Dict[str, str]:
        environment = {key: value for key, value in os.environ.items() if not key.startswith("NEURON_")}
        environment.update(extra)
        return environment

    def setUp(self) -> None:
        self.work = Path(tempfile.mkdtemp(dir=self.root))
        self.prefix = self.work / "prefix"
        self.cache = self.work / "cache"

    def neuron_env(self, base: str, **extra: str) -> Dict[str, str]:
        return self.base_env(NEURON_DOWNLOAD_BASE=base, NEURON_CACHE_DIR=str(self.cache), **extra)

    def install(self, *flags: str) -> Path:
        result = npm("install", "--global", "--prefix", str(self.prefix), *flags, str(self.tarball), cwd=self.work, env=self.base_env())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        if os.name == "nt":
            return self.prefix / "node_modules" / "@zero-x.live" / "neuron"
        return self.prefix / "lib" / "node_modules" / "@zero-x.live" / "neuron"

    def run_neuron(self, args: List[str], env: Dict[str, str]) -> subprocess.CompletedProcess:
        command = self.prefix / "neuron.cmd" if os.name == "nt" else self.prefix / "bin" / "neuron"
        return subprocess.run(
            [str(command), *args], env=env, capture_output=True, text=True, timeout=180, check=False
        )

    def test_tarball_installed_without_scripts_downloads_on_first_run(self) -> None:
        package = self.install("--ignore-scripts")
        for name in ("LICENSE", "NOTICE", "README.md"):
            self.assertTrue((package / name).is_file(), name)
        self.assertFalse((package / "test").exists())

        self.server.requests.clear()
        first = self.run_neuron(["--version"], self.neuron_env(self.server.base_url))
        self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
        self.assertIn(f"neuron-dummy {VERSION}", first.stdout)
        self.assertIn("Downloading Neuron", first.stderr)
        asset = fake_release.current_asset()
        self.assertEqual(self.server.paths(), [f"/v{VERSION}/{asset}.sha256", f"/v{VERSION}/{asset}"])

        offline = self.run_neuron(["prompt", "two words"], self.neuron_env(fake_release.unused_loopback_url(), NEURON_DUMMY_EXIT="7"))
        self.assertEqual(offline.returncode, 7, offline.stdout + offline.stderr)
        self.assertIn("neuron-dummy args: prompt two words", offline.stdout)
        self.assertNotIn("Downloading", offline.stderr)

    def test_postinstall_installs_a_verified_binary_next_to_the_launcher(self) -> None:
        package = self.install("--ignore-scripts")
        # Runs only this package's postinstall, not the dependencies' install scripts.
        rebuilt = npm(
            "rebuild", "--global", "--prefix", str(self.prefix), "--foreground-scripts", PACKAGE_NAME,
            cwd=self.work,
            env=self.neuron_env(self.server.base_url),
        )
        self.assertEqual(rebuilt.returncode, 0, rebuilt.stdout + rebuilt.stderr)
        native = package / "bin" / "native" / ("neuron.exe" if os.name == "nt" else "neuron")
        self.assertTrue(native.is_file())
        self.assertTrue(Path(f"{native}.sha256").is_file())

        offline = self.run_neuron(["--version"], self.neuron_env(fake_release.unused_loopback_url()))
        self.assertEqual(offline.returncode, 0, offline.stdout + offline.stderr)
        self.assertIn(f"neuron-dummy {VERSION}", offline.stdout)
        self.assertFalse(self.cache.exists(), "the packaged binary must be used before the user cache")


if __name__ == "__main__":
    unittest.main()
