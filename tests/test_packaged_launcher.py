from __future__ import annotations

import hashlib
import importlib.util
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import ModuleType
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fake_release  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parents[1]
LAUNCHER_SOURCE = REPO_ROOT / "neuron_cli" / "__init__.py"


def install_launcher(root: Path) -> Path:
    """Copy the launcher into an installed-style layout and return site-packages."""
    site = root / "site-packages"
    package = site / "neuron_cli"
    package.mkdir(parents=True)
    shutil.copyfile(LAUNCHER_SOURCE, package / "__init__.py")
    return site


def load_launcher(site: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location("installed_neuron", site / "neuron_cli" / "__init__.py")
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class PackagedLauncherTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temporary = tempfile.TemporaryDirectory()
        self.root = Path(self._temporary.name)
        self.site = install_launcher(self.root)
        self.launcher = load_launcher(self.site)
        self.cache = self.root / "cache"

    def tearDown(self) -> None:
        self._temporary.cleanup()

    def test_installed_layout_ignores_other_path_shims_and_forwards_arguments(self) -> None:
        launcher = self.launcher
        stale = self.root / "old-scripts"
        stale.mkdir()
        (stale / "neuron").write_text("old shim", encoding="utf-8")
        (stale / "neuron.exe").write_text("old shim", encoding="utf-8")
        binary = self.root / "downloaded-version-matched-neuron"
        environment = {"PATH": str(stale), "NEURON_BINARY_PATH": "", "NEURON_CACHE_DIR": str(self.cache)}
        argv = [str(self.root / "scripts" / "neuron"), "prompt", "two words"]
        expected_args = [str(binary), "prompt", "two words"]
        with patch.dict(os.environ, environment), patch.object(
            launcher, "_download_binary", return_value=binary
        ) as download, patch.object(sys, "argv", argv):
            if os.name == "posix":
                # A real exec never returns; the mock must not either.
                with patch.object(launcher.os, "execv", side_effect=SystemExit(0)) as execv, patch.object(
                    launcher, "_run_child"
                ) as run, self.assertRaises(SystemExit):
                    launcher.main()
                execv.assert_called_once_with(str(binary), expected_args)
                run.assert_not_called()
            else:
                with patch.object(launcher, "_run_child", return_value=7) as run, self.assertRaises(SystemExit) as result:
                    launcher.main()
                self.assertEqual(result.exception.code, 7)
                run.assert_called_once_with(expected_args)
            version = launcher._package_version()
            download.assert_called_once_with(version, launcher._release_asset(), self.cache / version)

    def test_binary_path_override_must_be_an_existing_native_binary(self) -> None:
        native = self.root / "native-neuron"
        native.touch()
        with patch.dict(os.environ, {"NEURON_BINARY_PATH": str(native)}):
            self.assertEqual(self.launcher._find_binary(), native.resolve())
        with patch.dict(os.environ, {"NEURON_BINARY_PATH": str(self.root / "missing")}):
            with self.assertRaises(self.launcher.LauncherError):
                self.launcher._find_binary()

    def test_release_assets_cover_every_published_platform(self) -> None:
        cases = {
            ("Windows", "AMD64"): "neuron-windows-x64.exe",
            ("Windows", "ARM64"): "neuron-windows-x64.exe",
            ("Linux", "x86_64"): "neuron-linux-x64",
            ("Linux", "aarch64"): "neuron-linux-arm64",
            ("Darwin", "arm64"): "neuron-macos-arm64",
            ("Darwin", "x86_64"): "neuron-macos-x64",
        }
        for (system, machine), asset in cases.items():
            with self.subTest(system=system, machine=machine):
                self.assertEqual(self.launcher._release_asset(system, machine), asset)
        with self.assertRaises(self.launcher.LauncherError) as error:
            self.launcher._release_asset("FreeBSD", "amd64")
        self.assertIn("NEURON_BINARY_PATH", str(error.exception))

    def test_the_full_screen_ui_has_an_asset_beside_every_cli_asset(self) -> None:
        cases = {
            ("Windows", "AMD64"): "neuron-tui-windows-x64.exe",
            ("Windows", "ARM64"): "neuron-tui-windows-x64.exe",
            ("Linux", "x86_64"): "neuron-tui-linux-x64",
            ("Linux", "aarch64"): "neuron-tui-linux-arm64",
            ("Darwin", "arm64"): "neuron-tui-macos-arm64",
            ("Darwin", "x86_64"): "neuron-tui-macos-x64",
        }
        for (system, machine), asset in cases.items():
            with self.subTest(system=system, machine=machine):
                self.assertEqual(self.launcher._release_asset(system, machine, component="ui"), asset)

    def test_the_ui_is_wanted_only_for_a_bare_launch_on_a_terminal(self) -> None:
        wants = self.launcher._wants_full_screen_ui
        self.assertTrue(wants([], True, {}))
        self.assertTrue(wants(["--model", "opus", "--permission-mode", "read-only", "--allow-broad-cwd"], True, {}))
        self.assertFalse(wants([], False, {}))
        self.assertFalse(wants([], True, {"NEURON_NO_TUI": "1"}))
        self.assertTrue(wants([], True, {"NEURON_NO_TUI": "0"}))
        for args in (["--no-tui"], ["fix the bug"], ["-p", "hi"], ["status"], ["--help"], ["--version"]):
            with self.subTest(args=args):
                self.assertFalse(wants(args, True, {}))

    def test_off_a_terminal_the_ui_is_not_fetched(self) -> None:
        with patch.dict(os.environ, {"NEURON_DOWNLOAD_BASE": "http://127.0.0.1:9"}, clear=False):
            os.environ.pop("NEURON_TUI_BIN", None)
            with patch.object(self.launcher, "_download_binary") as download:
                with patch.object(self.launcher.sys.stdin, "isatty", return_value=False):
                    self.assertIsNone(self.launcher._find_ui_binary([]))
                download.assert_not_called()

    def test_download_base_must_be_https_or_loopback(self) -> None:
        for base in ("https://mirror.example/releases/download/", "http://127.0.0.1:8000", "http://localhost:1"):
            with self.subTest(base=base), patch.dict(os.environ, {"NEURON_DOWNLOAD_BASE": base}):
                self.assertEqual(self.launcher._download_base(), base.rstrip("/"))
        for base in ("http://mirror.example/releases", "file:///tmp/releases", "ftp://127.0.0.1/"):
            with self.subTest(base=base), patch.dict(os.environ, {"NEURON_DOWNLOAD_BASE": base}):
                with self.assertRaises(self.launcher.LauncherError):
                    self.launcher._download_base()

    def test_exit_status_translation(self) -> None:
        self.assertEqual(self.launcher._exit_status(0), 0)
        self.assertEqual(self.launcher._exit_status(7), 7)
        self.assertEqual(self.launcher._exit_status(-2), 130)
        if os.name == "nt":
            self.assertEqual(self.launcher._exit_status(0xC000013A) & 0xFFFFFFFF, 0xC000013A)

    def test_parent_ignores_interrupts_only_while_the_child_runs(self) -> None:
        launcher = self.launcher
        names = [name for name in ("SIGINT", "SIGBREAK") if hasattr(signal, name)]
        before = {name: signal.getsignal(getattr(signal, name)) for name in names}
        seen = {}

        class FakeProcess:
            def __init__(self, args):
                self.args = args

            def __enter__(self):
                return self

            def __exit__(self, *_exc):
                return False

            def wait(self):
                for name in names:
                    seen[name] = signal.getsignal(getattr(signal, name))
                return 3

        with patch.object(launcher.subprocess, "Popen", FakeProcess):
            self.assertEqual(launcher._run_child(["neuron", "--help"]), 3)
        expected = signal.SIG_IGN if os.name == "nt" else launcher._ignore_signal
        for name in names:
            self.assertEqual(seen[name], expected, name)
            self.assertEqual(signal.getsignal(getattr(signal, name)), before[name], name)

    def test_prerelease_versions_download_the_semver_tagged_release(self) -> None:
        # importlib.metadata would report 6.4.0rc1; the tag and assets are v6.4.0-rc.1.
        launcher = self.launcher
        version = "6.4.0-rc.1"
        asset = launcher._release_asset()
        releases = self.root / "releases"
        fake_release.stage_release(releases, version, asset, b"prerelease neuron")
        with fake_release.FakeReleaseServer(releases) as server, patch.object(
            launcher, "PACKAGE_VERSION", version
        ), patch.dict(
            os.environ,
            {"NEURON_BINARY_PATH": "", "NEURON_CACHE_DIR": str(self.cache), "NEURON_DOWNLOAD_BASE": server.base_url},
        ):
            binary = launcher._find_binary()
            requested = server.paths()
        self.assertEqual(requested, [f"/v{version}/{asset}.sha256", f"/v{version}/{asset}"])
        self.assertEqual(binary, self.cache / version / asset)
        self.assertEqual(binary.read_bytes(), b"prerelease neuron")

    def test_package_version_is_the_tag_spelling_not_the_installed_metadata(self) -> None:
        with patch("importlib.metadata.version", return_value="6.4.0rc1"):
            self.assertEqual(self.launcher._package_version(), self.launcher.PACKAGE_VERSION)

    def test_cached_binary_requires_a_valid_sidecar(self) -> None:
        launcher = self.launcher
        asset = "neuron-linux-x64"
        self.cache.mkdir()
        binary = self.cache / asset
        binary.write_bytes(b"binary")
        self.assertIsNone(launcher._cached_binary(self.cache, asset))
        (self.cache / f"{asset}.sha256").write_text("not-a-checksum\n", encoding="ascii")
        self.assertIsNone(launcher._cached_binary(self.cache, asset))
        (self.cache / f"{asset}.sha256").write_text(fake_release.checksum_line(b"binary", asset), encoding="ascii")
        self.assertEqual(launcher._cached_binary(self.cache, asset), binary)


@unittest.skipUnless(fake_release.can_build_dummy(), "needs csc.exe to build a Windows dummy binary")
class LauncherReleaseServerTests(unittest.TestCase):
    """Runs the installed launcher in a real interpreter against a fake release server."""

    def setUp(self) -> None:
        self._temporary = tempfile.TemporaryDirectory()
        self.root = Path(self._temporary.name)
        self.site = install_launcher(self.root)
        launcher = load_launcher(self.site)
        self.version = launcher._package_version()
        self.asset = launcher._release_asset()
        self.releases = self.root / "releases"
        self.cache = self.root / "cache"

    def tearDown(self) -> None:
        self._temporary.cleanup()

    def launcher_env(self, base: str, **extra_env: str) -> dict:
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("NEURON_") and key != "PYTHONPATH"
        }
        environment.update(
            PYTHONPATH=str(self.site),
            NEURON_CACHE_DIR=str(self.cache),
            NEURON_DOWNLOAD_BASE=base,
            **extra_env,
        )
        return environment

    def run_launcher(self, base: str, *args: str, **extra_env: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, "-c", "import neuron_cli; neuron_cli.main()", *args],
            env=self.launcher_env(base, **extra_env),
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )

    def cached_files(self) -> list:
        directory = self.cache / self.version
        return sorted(entry.name for entry in directory.iterdir()) if directory.exists() else []

    def test_first_download_verifies_caches_and_runs_the_binary(self) -> None:
        staged = fake_release.stage_release(self.releases, self.version, self.asset)
        with fake_release.FakeReleaseServer(self.releases) as server:
            result = self.run_launcher(server.base_url, "--version")
            requested = server.paths()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"neuron-dummy {self.version}", result.stdout)
        self.assertIn("Downloading Neuron", result.stderr)
        prefix = f"/v{self.version}/{self.asset}"
        self.assertEqual(requested, [f"{prefix}.sha256", prefix])
        self.assertEqual(self.cached_files(), [self.asset, f"{self.asset}.sha256"])
        digest = hashlib.sha256(staged.read_bytes()).hexdigest()
        sidecar = (self.cache / self.version / f"{self.asset}.sha256").read_text(encoding="ascii")
        self.assertEqual(sidecar, f"{digest}  {self.asset}\n")

    def test_relaunch_runs_offline_from_the_cache(self) -> None:
        fake_release.stage_release(self.releases, self.version, self.asset)
        with fake_release.FakeReleaseServer(self.releases) as server:
            first = self.run_launcher(server.base_url, "--version")
        self.assertEqual(first.returncode, 0, first.stderr)

        offline = self.run_launcher(fake_release.unused_loopback_url(), "--version")
        self.assertEqual(offline.returncode, 0, offline.stderr)
        self.assertIn(f"neuron-dummy {self.version}", offline.stdout)
        self.assertNotIn("Downloading", offline.stderr)

        with fake_release.FakeReleaseServer(self.releases) as server:
            again = self.run_launcher(server.base_url, "--version")
            self.assertEqual(server.paths(), [])
        self.assertEqual(again.returncode, 0, again.stderr)

    def test_checksum_mismatch_is_rejected_and_nothing_is_cached(self) -> None:
        wrong = f"{'0' * 64}  {self.asset}\n"
        fake_release.stage_release(self.releases, self.version, self.asset, checksum=wrong)
        with fake_release.FakeReleaseServer(self.releases) as server:
            result = self.run_launcher(server.base_url, "--version")
        self.assertEqual(result.returncode, 1)
        self.assertIn("failed its SHA-256 check", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        self.assertEqual(self.cached_files(), [])

    def test_first_launch_offline_prints_an_actionable_error(self) -> None:
        result = self.run_launcher(fake_release.unused_loopback_url(), "--version")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Could not download Neuron", result.stderr)
        self.assertIn("NEURON_BINARY_PATH", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_unpublished_release_reports_http_404(self) -> None:
        self.releases.mkdir()
        with fake_release.FakeReleaseServer(self.releases) as server:
            result = self.run_launcher(server.base_url, "--version")
        self.assertEqual(result.returncode, 1)
        self.assertIn("HTTP 404", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_missing_sidecar_triggers_a_verified_download(self) -> None:
        fake_release.stage_release(self.releases, self.version, self.asset)
        stale = self.cache / self.version / self.asset
        fake_release.write_executable(stale, b"partial download")
        with fake_release.FakeReleaseServer(self.releases) as server:
            result = self.run_launcher(server.base_url, "--version")
            self.assertEqual(len(server.paths()), 2)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.cached_files(), [self.asset, f"{self.asset}.sha256"])

    def test_arguments_and_exit_code_are_forwarded(self) -> None:
        fake_release.stage_release(self.releases, self.version, self.asset)
        with fake_release.FakeReleaseServer(self.releases) as server:
            result = self.run_launcher(server.base_url, "prompt", "two words", NEURON_DUMMY_EXIT="7")
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertIn("neuron-dummy args: prompt two words", result.stdout)

    @unittest.skipUnless(os.name == "posix", "os.execv is used on POSIX only")
    def test_posix_launcher_replaces_itself_with_the_binary(self) -> None:
        fake_release.stage_release(self.releases, self.version, self.asset)
        with fake_release.FakeReleaseServer(self.releases) as server:
            self.assertEqual(self.run_launcher(server.base_url, "--version").returncode, 0)
        process = subprocess.Popen(
            [sys.executable, "-c", "import neuron_cli; neuron_cli.main()", "status"],
            env=self.launcher_env(fake_release.unused_loopback_url(), NEURON_DUMMY_PID="1"),
            stdout=subprocess.PIPE,
            text=True,
        )
        output, _ = process.communicate(timeout=60)
        self.assertEqual(process.returncode, 0)
        self.assertIn(f"pid={process.pid}", output)


if __name__ == "__main__":
    unittest.main()
