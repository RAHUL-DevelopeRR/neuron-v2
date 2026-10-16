"""End-to-end tests for install.sh and install.ps1 against a local fake release server."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path, PureWindowsPath
from typing import Dict, List, Optional

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fake_release  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parents[1]
INSTALL_SH = REPO_ROOT / "install.sh"
INSTALL_PS1 = REPO_ROOT / "install.ps1"
VERSION = "6.3.0"

FAKE_UNAME = """#!/bin/sh
case "${1:-}" in
    -m) echo "$FAKE_UNAME_M" ;;
    *) echo "$FAKE_UNAME_S" ;;
esac
"""

FAILING_BINARY = b"#!/bin/sh\necho 'cannot execute binary file' >&2\nexit 3\n"


def posix_dummy(version: str) -> bytes:
    return fake_release.POSIX_DUMMY.format(version=version).encode()


def install_sh_shells() -> List[str]:
    """Shells to run install.sh with: sh and bash on POSIX, NEURON_TEST_BASH on Windows."""
    if os.name == "nt":
        configured = os.environ.get("NEURON_TEST_BASH")
        return [configured] if configured and Path(configured).is_file() else []
    return [shell for shell in ("sh", "bash") if shutil.which(shell)]


def shell_path(path: Path) -> str:
    """A path as the POSIX shell sees it (MSYS style on Windows)."""
    if os.name != "nt":
        return str(path)
    windows = PureWindowsPath(path)
    drive = windows.drive.rstrip(":").lower()
    return "/" + drive + "/" + "/".join(windows.parts[1:])


def clean_environment() -> Dict[str, str]:
    return {key: value for key, value in os.environ.items() if not key.startswith(("NEURON_", "FAKE_UNAME"))}


@unittest.skipUnless(install_sh_shells(), "needs sh/bash (set NEURON_TEST_BASH on Windows)")
class InstallShTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temporary = tempfile.TemporaryDirectory()
        self.root = Path(self._temporary.name)
        self.releases = self.root / "releases"
        self.releases.mkdir()
        self.install_dir = self.root / "home" / ".local" / "bin"
        self.fakebin = self.root / "fakebin"
        fake_release.write_executable(self.fakebin / "uname", FAKE_UNAME.encode())
        self.server = fake_release.FakeReleaseServer(self.releases).start()

    def tearDown(self) -> None:
        self.server.stop()
        self._temporary.cleanup()

    def run_installer(
        self,
        *args: str,
        uname: Optional[tuple] = ("Linux", "x86_64"),
        on_path: bool = False,
        shadow: Optional[Path] = None,
        shell: Optional[str] = None,
        **env: str,
    ) -> subprocess.CompletedProcess:
        environment = clean_environment()
        environment.update(
            HOME=shell_path(self.root / "home"),
            NEURON_DOWNLOAD_BASE=self.server.base_url,
            NEURON_RELEASES_API=f"{self.server.base_url}/repos/neuron/releases/latest",
            NEURON_INSTALL_DIR=shell_path(self.install_dir),
        )
        environment.pop("NO_COLOR", None)
        if uname is not None:
            environment["FAKE_UNAME_S"], environment["FAKE_UNAME_M"] = uname
            environment["PATH"] = os.pathsep.join([str(self.fakebin), environment.get("PATH", "")])
        environment.update(env)
        shell = shell or install_sh_shells()[0]
        command = [shell, str(INSTALL_SH), *args]
        if on_path:
            # Prepend inside the shell: MSYS rewrites Windows PATH entries (Temp becomes /tmp).
            prefix = [shell_path(self.install_dir)]
            if shadow is not None:
                prefix.insert(0, shell_path(shadow))
            environment["NEURON_TEST_SHELL"] = shell
            environment["NEURON_TEST_PATH_PREFIX"] = ":".join(prefix)
            script = 'PATH="$NEURON_TEST_PATH_PREFIX:$PATH"; export PATH; exec "$NEURON_TEST_SHELL" "$0" "$@"'
            command = [shell, "-c", script, str(INSTALL_SH), *args]
        return subprocess.run(
            command,
            env=environment,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )

    def installed(self) -> Path:
        return self.install_dir / "neuron"

    def run_installed(self, *args: str) -> str:
        result = subprocess.run(
            [install_sh_shells()[0], "-c", '"$0" "$@"', shell_path(self.installed()), *args],
            capture_output=True,
            text=True,
            timeout=60,
            check=True,
        )
        return result.stdout

    def test_installs_a_pinned_version_with_every_shell(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        for shell in install_sh_shells():
            with self.subTest(shell=shell):
                self.server.requests.clear()
                result = self.run_installer(shell=shell, NEURON_VERSION=VERSION)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f"Installing Neuron {VERSION} (linux-x64)", result.stderr)
                self.assertIn("verified SHA-256", result.stderr)
                self.assertIn("auth login", result.stderr)
                self.assertIn("is not on your PATH", result.stderr)
                self.assertEqual(
                    self.server.paths(),
                    [f"/v{VERSION}/neuron-linux-x64.sha256", f"/v{VERSION}/neuron-linux-x64"],
                )
                self.assertEqual(self.installed().read_bytes(), posix_dummy(VERSION))
                self.assertIn(f"neuron-dummy {VERSION}", self.run_installed("--version"))
                self.assertEqual(sorted(entry.name for entry in self.install_dir.iterdir()), ["neuron"])

    def test_command_line_options_override_the_environment(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        other = self.root / "custom" / "bin"
        result = self.run_installer("--version", f"v{VERSION}", "--install-dir", shell_path(other), NEURON_VERSION="1.0.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((other / "neuron").is_file())
        self.assertFalse(self.installed().exists())

    def test_resolves_the_latest_release_through_the_api(self) -> None:
        for version in ("6.3.0", "6.10.0"):
            fake_release.stage_release(self.releases, version, "neuron-linux-x64", posix_dummy(version))
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.server.paths()[0], "/repos/neuron/releases/latest")
        self.assertIn("neuron-dummy 6.10.0", self.run_installed("--version"))

    def test_selects_the_asset_for_each_supported_platform(self) -> None:
        cases = {
            ("Linux", "x86_64"): "neuron-linux-x64",
            ("Linux", "amd64"): "neuron-linux-x64",
            ("Linux", "aarch64"): "neuron-linux-arm64",
            ("Darwin", "arm64"): "neuron-macos-arm64",
            ("Darwin", "x86_64"): "neuron-macos-x64",
        }
        for asset in set(cases.values()):
            fake_release.stage_release(self.releases, VERSION, asset, posix_dummy(VERSION))
        for uname, asset in cases.items():
            with self.subTest(uname=uname):
                if uname == ("Darwin", "x86_64") and sys.platform == "darwin" and os.uname().machine == "arm64":
                    continue  # a real Apple silicon host is detected through sysctl, by design
                self.server.requests.clear()
                result = self.run_installer(uname=uname, NEURON_VERSION=VERSION)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.server.paths()[-1], f"/v{VERSION}/{asset}")

    @unittest.skipIf(os.name == "nt", "real platform detection needs a POSIX host")
    def test_detects_the_real_host_platform(self) -> None:
        asset = fake_release.current_asset()
        fake_release.stage_release(self.releases, VERSION, asset, posix_dummy(VERSION))
        result = self.run_installer(uname=None, NEURON_VERSION=VERSION)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.server.paths()[-1], f"/v{VERSION}/{asset}")

    def test_unsupported_platforms_fail_before_any_download(self) -> None:
        cases = {
            ("FreeBSD", "amd64"): "unsupported operating system: FreeBSD",
            ("Linux", "riscv64"): "unsupported CPU architecture: riscv64",
            ("MINGW64_NT-10.0-26100", "x86_64"): "install.ps1",
            ("CYGWIN_NT-10.0", "x86_64"): "install.ps1",
        }
        for uname, message in cases.items():
            with self.subTest(uname=uname):
                result = self.run_installer(uname=uname, NEURON_VERSION=VERSION)
                self.assertEqual(result.returncode, 1)
                self.assertIn(message, result.stderr)
        self.assertEqual(self.server.paths(), [])
        self.assertFalse(self.installed().exists())

    def test_checksum_mismatch_keeps_the_existing_install(self) -> None:
        fake_release.write_executable(self.installed(), b"previous neuron")
        fake_release.stage_release(
            self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION), checksum=f"{'a' * 64}  neuron-linux-x64\n"
        )
        result = self.run_installer(NEURON_VERSION=VERSION)
        self.assertEqual(result.returncode, 1)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertIn("Nothing was installed", result.stderr)
        self.assertEqual(self.installed().read_bytes(), b"previous neuron")

    def test_malformed_checksum_file_is_rejected(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION), checksum="oops\n")
        result = self.run_installer(NEURON_VERSION=VERSION)
        self.assertEqual(result.returncode, 1)
        self.assertIn("does not contain a SHA-256 checksum", result.stderr)

    def test_binary_that_cannot_run_is_not_installed(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", FAILING_BINARY)
        result = self.run_installer(NEURON_VERSION=VERSION)
        self.assertEqual(result.returncode, 1)
        self.assertIn("does not run on this system", result.stderr)
        self.assertFalse(self.installed().exists())

    def test_unpublished_version_points_at_the_releases_page(self) -> None:
        result = self.run_installer(NEURON_VERSION="9.9.9")
        self.assertEqual(result.returncode, 1)
        self.assertIn("may not be published", result.stderr)
        self.assertIn("releases", result.stderr)

    def test_rejects_insecure_download_base_and_invalid_versions(self) -> None:
        insecure = self.run_installer(NEURON_VERSION=VERSION, NEURON_DOWNLOAD_BASE="http://downloads.example.com")
        self.assertEqual(insecure.returncode, 1)
        self.assertIn("must be an HTTPS URL", insecure.stderr)
        invalid = self.run_installer(NEURON_VERSION="6.3.0;touch pwned")
        self.assertEqual(invalid.returncode, 1)
        self.assertIn("invalid version", invalid.stderr)
        self.assertEqual(self.server.paths(), [])

    def test_upgrade_on_path_replaces_the_binary_without_a_path_warning(self) -> None:
        fake_release.write_executable(self.installed(), posix_dummy("6.2.5"))
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        result = self.run_installer(on_path=True, NEURON_VERSION=VERSION)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("not on your PATH", result.stderr)
        self.assertIn("  neuron auth login", result.stderr)
        self.assertIn(f"neuron-dummy {VERSION}", self.run_installed("--version"))

    def test_warns_when_another_neuron_shadows_the_install(self) -> None:
        shadow = self.root / "python" / "Scripts"
        fake_release.write_executable(shadow / "neuron", posix_dummy("6.2.4"))
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        result = self.run_installer(on_path=True, shadow=shadow, NEURON_VERSION=VERSION)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("comes first on PATH", result.stderr)
        self.assertIn("pip uninstall neuroncli", result.stderr)

    def test_help_and_unknown_arguments(self) -> None:
        help_result = self.run_installer("--help")
        self.assertEqual(help_result.returncode, 0)
        self.assertIn("Usage: install.sh", help_result.stdout)
        unknown = self.run_installer("--bogus")
        self.assertEqual(unknown.returncode, 1)
        self.assertIn("unknown argument: --bogus", unknown.stderr)


def powershell_hosts() -> List[str]:
    if os.name != "nt":
        return []
    hosts = [host for host in ("powershell", "pwsh") if shutil.which(host)]
    return hosts


@unittest.skipUnless(powershell_hosts() and fake_release.can_build_dummy(), "install.ps1 tests need Windows PowerShell and csc.exe")
class InstallPs1Tests(unittest.TestCase):
    ASSET = "neuron-windows-x64.exe"

    def setUp(self) -> None:
        self._temporary = tempfile.TemporaryDirectory()
        self.root = Path(self._temporary.name)
        self.releases = self.root / "releases"
        self.releases.mkdir()
        self.install_dir = self.root / "Programs" / "Neuron" / "bin"
        self.server = fake_release.FakeReleaseServer(self.releases).start()

    def tearDown(self) -> None:
        self.server.stop()
        self._temporary.cleanup()

    def environment(self, **env: str) -> Dict[str, str]:
        environment = clean_environment()
        environment.update(
            NEURON_DOWNLOAD_BASE=self.server.base_url,
            NEURON_RELEASES_API=f"{self.server.base_url}/repos/neuron/releases/latest",
            LOCALAPPDATA=str(self.root),
        )
        environment.update(env)
        return environment

    def run_installer(self, *args: str, host: str = "powershell", **env: str) -> subprocess.CompletedProcess:
        environment = self.environment(**env)
        for key in [key for key, value in environment.items() if value == ""]:
            del environment[key]  # an empty value would corrupt the Windows environment block
        return subprocess.run(
            [host, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(INSTALL_PS1), *args],
            env=environment,
            capture_output=True,
            text=True,
            timeout=180,
            check=False,
        )

    def installed(self) -> Path:
        return self.install_dir / "neuron.exe"

    @staticmethod
    def user_path() -> Optional[str]:
        import winreg

        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment") as key:
            try:
                return winreg.QueryValueEx(key, "Path")[0]
            except FileNotFoundError:
                return None

    def test_installs_into_local_app_data_with_every_powershell(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        before = self.user_path()
        for host in powershell_hosts():
            with self.subTest(host=host):
                self.server.requests.clear()
                result = self.run_installer("-Version", VERSION, host=host)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(f"Installing Neuron {VERSION} (windows-x64)", result.stdout)
                self.assertIn("verified SHA-256", result.stdout)
                self.assertIn("auth login", result.stdout)
                self.assertIn("is not on your PATH", result.stdout)
                self.assertEqual(
                    self.server.paths(), [f"/v{VERSION}/{self.ASSET}.sha256", f"/v{VERSION}/{self.ASSET}"]
                )
                output = subprocess.run([str(self.installed()), "--version"], capture_output=True, text=True, check=True)
                self.assertIn(f"neuron-dummy {VERSION}", output.stdout)
        self.assertEqual(self.user_path(), before, "install.ps1 must not touch the user PATH without -AddToPath")

    def test_environment_variables_and_latest_release_lookup(self) -> None:
        for version in ("6.3.0", "6.4.1"):
            fake_release.stage_release(self.releases, version, self.ASSET)
        custom = self.root / "custom"
        result = self.run_installer(NEURON_INSTALL_DIR=str(custom))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.server.paths()[0], "/repos/neuron/releases/latest")
        self.assertIn("Installing Neuron 6.4.1", result.stdout)
        self.assertTrue((custom / "neuron.exe").is_file())

        pinned = self.run_installer(NEURON_INSTALL_DIR=str(custom), NEURON_VERSION="v6.3.0")
        self.assertEqual(pinned.returncode, 0, pinned.stdout + pinned.stderr)
        self.assertIn("Installing Neuron 6.3.0", pinned.stdout)

    def test_checksum_mismatch_installs_nothing(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET, checksum=f"{'b' * 64}  {self.ASSET}\n")
        result = self.run_installer("-Version", VERSION)
        self.assertEqual(result.returncode, 1)
        self.assertIn("checksum mismatch", result.stdout)
        self.assertFalse(self.installed().exists())

    def test_unpublished_version_and_insecure_base_fail_clearly(self) -> None:
        missing = self.run_installer("-Version", "9.9.9")
        self.assertEqual(missing.returncode, 1)
        self.assertIn("may not be published", missing.stdout)
        insecure = self.run_installer("-Version", VERSION, NEURON_DOWNLOAD_BASE="http://downloads.example.com")
        self.assertEqual(insecure.returncode, 1)
        self.assertIn("must be an HTTPS URL", insecure.stdout)

    def test_unsupported_architecture_is_rejected(self) -> None:
        environment = {"PROCESSOR_ARCHITECTURE": "x86", "PROCESSOR_ARCHITEW6432": ""}  # 32-bit host
        result = self.run_installer("-Version", VERSION, **environment)
        self.assertEqual(result.returncode, 1)
        self.assertIn("unsupported CPU architecture: x86", result.stdout)
        self.assertEqual(self.server.paths(), [])

    def test_reinstall_replaces_the_existing_binary(self) -> None:
        fake_release.write_executable(self.installed(), b"previous neuron")
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        result = self.run_installer("-Version", VERSION)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotEqual(self.installed().read_bytes(), b"previous neuron")
        self.assertFalse(Path(f"{self.installed()}.old").exists())

    def test_script_block_invocation_never_exits_the_host(self) -> None:
        command = (
            f"try {{ & ([scriptblock]::Create((Get-Content -Raw '{INSTALL_PS1}'))) -Version 9.9.9 }} "
            "catch { 'caught-install-error' }; 'host-still-running'"
        )
        result = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", command],
            env=self.environment(),
            capture_output=True,
            text=True,
            timeout=180,
            check=False,
        )
        self.assertIn("caught-install-error", result.stdout)
        self.assertIn("host-still-running", result.stdout)

    @unittest.skipUnless(os.environ.get("GITHUB_ACTIONS") == "true", "changes the user PATH; runs only on CI runners")
    def test_add_to_path_updates_the_user_path_once(self) -> None:
        import winreg

        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment") as key:
            try:
                original = winreg.QueryValueEx(key, "Path")
            except FileNotFoundError:
                original = None
        try:
            for _attempt in range(2):
                result = self.run_installer("-Version", VERSION, "-AddToPath")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            entries = [entry.rstrip("\\").lower() for entry in (self.user_path() or "").split(";")]
            self.assertEqual(entries.count(str(self.install_dir).lower()), 1)
        finally:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment", 0, winreg.KEY_SET_VALUE) as key:
                if original is None:
                    winreg.DeleteValue(key, "Path")
                else:
                    winreg.SetValueEx(key, "Path", 0, original[1], original[0])


if __name__ == "__main__":
    unittest.main()
