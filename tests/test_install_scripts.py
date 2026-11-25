"""End-to-end tests for install.sh and install.ps1 against a local fake release server."""

from __future__ import annotations

import os
import re
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

# Reads stdin to the end, as a binary that probes the terminal might.
STDIN_READING_BINARY = b"#!/bin/sh\ncat >/dev/null\necho 'neuron-dummy stdin-reader'\n"

# Reports the path it was started from.
WHERE_BINARY = b'#!/bin/sh\necho "ran-from $0"\n'

GITHUB_TOKEN = "ghs_fakeTokenForTests123"

# Stands in for curl on api.github.com (records argv and the config read from stdin)
# and hands every other URL to the real curl.
FAKE_CURL = r"""#!/bin/sh
out=""
url=""
previous=""
for argument in "$@"; do
    if [ "$previous" = "-o" ]; then out="$argument"; fi
    previous="$argument"
    url="$argument"
done
case "$url" in
    https://api.github.com/*)
        printf '%s\n' "$*" >> "$FAKE_CURL_ARGV"
        case " $* " in
            *" --config - "*) cat >> "$FAKE_CURL_STDIN" ;;
        esac
        printf '{"tag_name": "v%s"}\n' "$FAKE_CURL_LATEST" > "$out"
        printf '200'  # the --write-out '%{http_code}' that install.sh checks
        exit 0
        ;;
esac
own_dir="${0%/*}"
remaining=""
old_ifs="$IFS"
IFS=:
for entry in $PATH; do
    if [ "$entry" != "$own_dir" ]; then remaining="${remaining:+$remaining:}$entry"; fi
done
IFS="$old_ifs"
PATH="$remaining"
export PATH
exec curl "$@"
"""


def stamp_release_version(script: str, version: str) -> str:
    """What .github/workflows/release.yml does to install.sh and install.ps1 before uploading them."""
    stamped, ps1 = re.subn(r"(?m)^\$NeuronReleaseVersion = ''$", f"$NeuronReleaseVersion = '{version}'", script)
    stamped, sh = re.subn(r'(?m)^RELEASE_VERSION=""$', f'RELEASE_VERSION="{version}"', stamped)
    assert ps1 + sh == 1, "the script has no release-version line to stamp"
    return stamped


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
        piped: bool = False,
        **env: str,
    ) -> subprocess.CompletedProcess:
        environment = clean_environment()
        environment.update(
            HOME=shell_path(self.root / "home"),
            NEURON_DOWNLOAD_BASE=self.server.base_url,
            NEURON_CHECKSUMS_BASE=self.server.base_url,
            NEURON_RELEASES_API=f"{self.server.base_url}/repos/neuron/releases/latest",
            NEURON_INSTALL_DIR=shell_path(self.install_dir),
            # The full-screen UI step has its own tests; NEURON_NO_TUI="" turns it on.
            NEURON_NO_TUI="1",
        )
        environment.pop("NO_COLOR", None)
        if uname is not None:
            environment["FAKE_UNAME_S"], environment["FAKE_UNAME_M"] = uname
            environment["PATH"] = os.pathsep.join([str(self.fakebin), environment.get("PATH", "")])
        environment.update(env)
        for key in [key for key, value in environment.items() if value == ""]:
            del environment[key]  # "" means unset (install.sh treats both alike)
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
        if piped:
            # Like `curl -fsSL .../install.sh | sh -s -- ARGS`: the script arrives on stdin.
            command = [shell, "-s", "--", *args]
        return subprocess.run(
            command,
            input=INSTALL_SH.read_text(encoding="utf-8") if piped else None,
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
                self.assertIn("neuron login", result.stderr)
                self.assertIn("is not on your PATH", result.stderr)
                self.assertEqual(
                    self.server.paths(),
                    [f"/v{VERSION}/SHA256SUMS", f"/v{VERSION}/neuron-linux-x64.sha256", f"/v{VERSION}/neuron-linux-x64"],
                )
                self.assertEqual(self.installed().read_bytes(), posix_dummy(VERSION))
                self.assertIn(f"neuron-dummy {VERSION}", self.run_installed("--version"))
                self.assertEqual(sorted(entry.name for entry in self.install_dir.iterdir()), ["neuron"])

    def test_installs_the_full_screen_ui_beside_neuron(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        fake_release.stage_release(self.releases, VERSION, "neuron-tui-linux-x64", posix_dummy(VERSION))
        result = self.run_installer(NEURON_VERSION=VERSION, NEURON_NO_TUI="")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("installed", result.stderr)
        self.assertEqual(sorted(entry.name for entry in self.install_dir.iterdir()), ["neuron", "neuron-tui"])
        self.assertEqual(
            self.server.paths(),
            [
                f"/v{VERSION}/SHA256SUMS",
                f"/v{VERSION}/neuron-linux-x64.sha256",
                f"/v{VERSION}/neuron-linux-x64",
                f"/v{VERSION}/neuron-tui-linux-x64.sha256",
                f"/v{VERSION}/neuron-tui-linux-x64",
            ],
        )

    def test_a_missing_or_bad_ui_still_installs_neuron(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        missing = self.run_installer(NEURON_VERSION=VERSION, NEURON_NO_TUI="")
        self.assertEqual(missing.returncode, 0, missing.stderr)
        self.assertIn("full-screen UI was not installed", missing.stderr)
        self.assertEqual(sorted(entry.name for entry in self.install_dir.iterdir()), ["neuron"])

        fake_release.stage_release(
            self.releases, VERSION, "neuron-tui-linux-x64", posix_dummy(VERSION), checksum="0" * 64 + "  neuron-tui-linux-x64\n"
        )
        bad = self.run_installer(NEURON_VERSION=VERSION, NEURON_NO_TUI="")
        self.assertEqual(bad.returncode, 0, bad.stderr)
        self.assertIn("checksum mismatch for neuron-tui-linux-x64", bad.stderr)
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

    def test_release_sums_that_disagree_or_are_missing_fail_closed(self) -> None:
        fake_release.write_executable(self.installed(), b"previous neuron")
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        sums = self.releases / f"v{VERSION}" / "SHA256SUMS"
        sums.write_text(f"{'c' * 64}  neuron-linux-x64\n", encoding="ascii")
        wrong = self.run_installer(NEURON_VERSION=VERSION)
        self.assertEqual(wrong.returncode, 1)
        self.assertIn("SHA256SUMS", wrong.stderr)
        self.assertIn("Nothing was installed", wrong.stderr)
        sums.write_text(f"{'c' * 64}  some-other-asset\n", encoding="ascii")
        unlisted = self.run_installer(NEURON_VERSION=VERSION)
        self.assertEqual(unlisted.returncode, 1)
        sums.unlink()
        missing = self.run_installer(NEURON_VERSION=VERSION)
        self.assertEqual(missing.returncode, 1)
        self.assertEqual(self.installed().read_bytes(), b"previous neuron")

    def test_checksums_come_from_their_own_base_not_the_mirror(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        result = self.run_installer(NEURON_VERSION=VERSION, NEURON_CHECKSUMS_BASE=fake_release.unused_loopback_url())
        self.assertEqual(result.returncode, 1)
        self.assertNotIn(f"/v{VERSION}/SHA256SUMS", self.server.paths())
        insecure = self.run_installer(NEURON_VERSION=VERSION, NEURON_CHECKSUMS_BASE="http://downloads.example.com")
        self.assertEqual(insecure.returncode, 1)
        self.assertIn("NEURON_CHECKSUMS_BASE must be an HTTPS URL", insecure.stderr)

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

    def test_piped_install_is_not_cut_short_by_a_binary_that_reads_stdin(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", STDIN_READING_BINARY)
        result = self.run_installer(piped=True, NEURON_VERSION=VERSION)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Installed", result.stderr)
        self.assertIn("Next steps:", result.stderr)
        self.assertEqual(self.installed().read_bytes(), STDIN_READING_BINARY)

    def test_every_curl_call_refuses_a_redirect_away_from_https(self) -> None:
        # A mirror or release redirecting to plain HTTP must not be followed (--proto-redir
        # only limits redirects; a loopback http base given on purpose still works).
        calls = [
            line.strip()
            for line in INSTALL_SH.read_text(encoding="utf-8").splitlines()
            if line.lstrip().startswith(("curl ", "checked_curl ")) and "--fail" in line
        ]
        self.assertGreaterEqual(len(calls), 2, calls)
        for call in calls:
            self.assertIn("--proto-redir =https", call)

    def test_unpublished_version_points_at_the_releases_page(self) -> None:
        result = self.run_installer(NEURON_VERSION="9.9.9")
        self.assertEqual(result.returncode, 1)
        self.assertIn("may not be published", result.stderr)
        self.assertIn("releases", result.stderr)

    def test_network_failure_is_not_reported_as_a_missing_release(self) -> None:
        result = self.run_installer(NEURON_VERSION=VERSION, NEURON_DOWNLOAD_BASE=fake_release.unused_loopback_url(), NEURON_CHECKSUMS_BASE=fake_release.unused_loopback_url())
        self.assertEqual(result.returncode, 1)
        self.assertIn("Check your network", result.stderr)
        self.assertNotIn("may not be published", result.stderr)
        self.assertFalse(self.installed().exists())

    def test_rejects_userinfo_and_lookalike_loopback_hosts(self) -> None:
        for base in (
            "http://127.0.0.1:x@evil.example/",
            "http://127.0.0.1@evil.example",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example:8000",
            "http://[::1].evil.example",
            "http://127.0.0.1:80x",
            "https://user@mirror.example",
        ):
            with self.subTest(base=base):
                result = self.run_installer(NEURON_VERSION=VERSION, NEURON_DOWNLOAD_BASE=base)
                self.assertEqual(result.returncode, 1)
                self.assertIn("must be an HTTPS URL", result.stderr)
        self.assertEqual(self.server.paths(), [])

    def test_github_token_reaches_only_the_github_api_and_never_argv(self) -> None:
        fake_release.write_executable(self.fakebin / "curl", FAKE_CURL.encode())
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        argv_log, stdin_log = self.root / "curl-argv", self.root / "curl-stdin"
        argv_log.touch()
        stdin_log.touch()
        result = self.run_installer(
            GITHUB_TOKEN=GITHUB_TOKEN,
            NEURON_RELEASES_API="",  # empty means the default, https://api.github.com/...
            FAKE_CURL_ARGV=shell_path(argv_log),
            FAKE_CURL_STDIN=shell_path(stdin_log),
            FAKE_CURL_LATEST=VERSION,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = argv_log.read_text(encoding="utf-8").splitlines()
        self.assertEqual(len(calls), 1, calls)
        self.assertIn("https://api.github.com/repos/RAHUL-DevelopeRR/neuron-v2/releases?per_page=10", calls[0])
        self.assertNotIn("releases/latest", calls[0])
        self.assertNotIn(GITHUB_TOKEN, calls[0])
        self.assertEqual(stdin_log.read_text(encoding="utf-8"), f'header = "Authorization: Bearer {GITHUB_TOKEN}"\n')
        self.assertEqual(self.server.paths(), [f"/v{VERSION}/SHA256SUMS", f"/v{VERSION}/neuron-linux-x64.sha256", f"/v{VERSION}/neuron-linux-x64"])
        self.assertEqual(self.server.authorization, [None, None, None])

    def test_github_token_is_not_sent_to_another_releases_api(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        result = self.run_installer(GITHUB_TOKEN=GITHUB_TOKEN)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.server.paths()[0], "/repos/neuron/releases/latest")
        # The releases lookup, then SHA256SUMS, the .sha256 file and the binary: none carries the token.
        self.assertEqual(self.server.authorization, [None, None, None, None])

    def test_self_check_runs_the_binary_from_the_install_directory(self) -> None:
        # The temporary directory may be mounted noexec; the binary has to run from the install directory anyway.
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", WHERE_BINARY)
        result = self.run_installer(NEURON_VERSION=VERSION)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"ran-from {shell_path(self.install_dir)}/.neuron.", result.stderr)
        self.assertEqual(sorted(entry.name for entry in self.install_dir.iterdir()), ["neuron"])

    def test_reports_the_version_line_of_the_release_binary(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        result = self.run_installer(NEURON_VERSION=VERSION, NEURON_DUMMY_VERSION_REPORT="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"\n  neuron {VERSION}\n", result.stderr)
        self.assertNotIn("\n  Neuron\n", result.stderr)

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
        self.assertIn("  neuron login", result.stderr)
        self.assertIn(f"neuron-dummy {VERSION}", self.run_installed("--version"))

    def test_warns_when_another_neuron_shadows_the_install(self) -> None:
        shadow = self.root / "python" / "Scripts"
        fake_release.write_executable(shadow / "neuron", posix_dummy("6.2.4"))
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        result = self.run_installer(on_path=True, shadow=shadow, NEURON_VERSION=VERSION)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("comes first on PATH", result.stderr)
        self.assertIn("pip uninstall neuroncli", result.stderr)
        self.assertIn("npm uninstall -g @zero-x.live/neuron neuron-cli-app", result.stderr)

    def test_windows_shell_hint_names_the_requested_version(self) -> None:
        windows_shell = ("MINGW64_NT-10.0-26100", "x86_64")
        pinned = self.run_installer(uname=windows_shell, NEURON_VERSION="v7.0.0-alpha.1")
        self.assertEqual(pinned.returncode, 1)
        self.assertIn(
            "& ([scriptblock]::Create((irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/"
            "v7.0.0-alpha.1/install.ps1))) -Version 7.0.0-alpha.1",
            pinned.stderr,
        )
        self.assertNotIn("releases/latest", pinned.stderr)
        unnamed = self.run_installer(uname=windows_shell)
        self.assertEqual(unnamed.returncode, 1)
        self.assertIn("https://github.com/RAHUL-DevelopeRR/neuron-v2/releases", unnamed.stderr)
        self.assertNotIn("releases/latest", unnamed.stderr)
        self.assertEqual(self.server.paths(), [])

    def test_newest_release_comes_from_the_release_list_and_skips_drafts(self) -> None:
        # GitHub's /releases/latest is a 404 while every release is a prerelease.
        for version in ("6.3.0", "6.4.1"):
            fake_release.stage_release(self.releases, version, "neuron-linux-x64", posix_dummy(version))
        self.server.draft_tags = ["v9.0.0"]
        result = self.run_installer(NEURON_RELEASES_API=f"{self.server.base_url}/repos/neuron/releases?per_page=10")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.server.paths()[0], "/repos/neuron/releases")
        self.assertIn("Installing Neuron 6.4.1", result.stderr)

    def test_a_release_stamped_script_needs_no_lookup(self) -> None:
        fake_release.stage_release(self.releases, VERSION, "neuron-linux-x64", posix_dummy(VERSION))
        stamped = stamp_release_version(INSTALL_SH.read_text(encoding="utf-8"), VERSION)
        script = self.root / "install.sh"
        script.write_text(stamped, encoding="utf-8", newline="\n")
        environment = clean_environment()
        environment.update(
            HOME=shell_path(self.root / "home"),
            NEURON_DOWNLOAD_BASE=self.server.base_url,
            NEURON_CHECKSUMS_BASE=self.server.base_url,
            NEURON_RELEASES_API=fake_release.unused_loopback_url(),
            NEURON_INSTALL_DIR=shell_path(self.install_dir),
            NEURON_NO_TUI="1",
            FAKE_UNAME_S="Linux",
            FAKE_UNAME_M="x86_64",
            PATH=os.pathsep.join([str(self.fakebin), os.environ.get("PATH", "")]),
        )
        result = subprocess.run(
            [install_sh_shells()[0], str(script)], env=environment, capture_output=True, text=True, timeout=120, check=False
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"Installing Neuron {VERSION}", result.stderr)
        self.assertEqual(self.server.paths()[0], f"/v{VERSION}/SHA256SUMS")

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
            NEURON_CHECKSUMS_BASE=self.server.base_url,
            NEURON_RELEASES_API=f"{self.server.base_url}/repos/neuron/releases/latest",
            LOCALAPPDATA=str(self.root),
            # The full-screen UI step has its own tests; NEURON_NO_TUI="" turns it on.
            NEURON_NO_TUI="1",
            # Adding to the user PATH is the default; the tests never write the real registry.
            NEURON_NO_MODIFY_PATH="1",
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

    def test_installs_into_local_app_data_with_every_powershell_and_downloader(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        before = self.user_path()
        for host in powershell_hosts():
            for downloader in ("curl", "dotnet"):
                with self.subTest(host=host, downloader=downloader):
                    self.server.requests.clear()
                    result = self.run_installer("-Version", VERSION, host=host, NEURON_DOWNLOADER=downloader)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertIn(f"Installing Neuron {VERSION} (windows-x64)", result.stdout)
                    self.assertIn("verified SHA-256", result.stdout)
                    # The binary's download reports its size and time.
                    self.assertRegex(result.stdout, r"neuron\.exe: \d+\.\d MB in \d+ s")
                    self.assertIn("login", result.stdout)
                    self.assertIn("is not on your PATH", result.stdout)
                    self.assertNotIn("curl.exe could not download", result.stdout)
                    self.assertEqual(
                        self.server.paths(),
                        [f"/v{VERSION}/SHA256SUMS", f"/v{VERSION}/{self.ASSET}.sha256", f"/v{VERSION}/{self.ASSET}"],
                    )
                    output = subprocess.run([str(self.installed()), "--version"], capture_output=True, text=True, check=True)
                    self.assertIn(f"neuron-dummy {VERSION}", output.stdout)
        self.assertEqual(self.user_path(), before, "the tests set NEURON_NO_MODIFY_PATH=1; the user PATH must not change")


    def test_installs_the_full_screen_ui_beside_neuron(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        fake_release.stage_release(self.releases, VERSION, "neuron-tui-windows-x64.exe")
        result = self.run_installer("-Version", VERSION, NEURON_NO_TUI="")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("neuron-tui.exe", result.stdout)
        self.assertTrue((self.install_dir / "neuron-tui.exe").is_file())
        self.assertEqual(
            self.server.paths()[-2:],
            [f"/v{VERSION}/neuron-tui-windows-x64.exe.sha256", f"/v{VERSION}/neuron-tui-windows-x64.exe"],
        )

    def test_a_missing_ui_still_installs_neuron(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        result = self.run_installer("-Version", VERSION, NEURON_NO_TUI="")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("full-screen UI was not installed", result.stdout)
        self.assertTrue(self.installed().is_file())
        self.assertFalse((self.install_dir / "neuron-tui.exe").exists())

    def test_newest_release_comes_from_the_release_list_and_skips_drafts(self) -> None:
        # GitHub's /releases/latest is a 404 while every release is a prerelease.
        for version in ("6.3.0", "6.4.1"):
            fake_release.stage_release(self.releases, version, self.ASSET)
        self.server.draft_tags = ["v9.0.0"]
        result = self.run_installer(NEURON_RELEASES_API=f"{self.server.base_url}/repos/neuron/releases?per_page=10")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.server.paths()[0], "/repos/neuron/releases")
        self.assertIn("Installing Neuron 6.4.1", result.stdout)

    def test_the_default_lookup_is_the_release_list_not_latest(self) -> None:
        text = INSTALL_PS1.read_text(encoding="utf-8")
        self.assertIn("https://api.github.com/repos/$NeuronRepository/releases?per_page=10", text)
        self.assertNotIn("releases/latest/download", text)
        self.assertNotIn('releases/latest"', text)

    def test_a_release_stamped_script_needs_no_lookup(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        script = self.root / "install.ps1"
        script.write_text(stamp_release_version(INSTALL_PS1.read_text(encoding="utf-8"), VERSION), encoding="utf-8")
        command = f"Get-Content -Raw '{script}' | Invoke-Expression"
        result = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", command],
            env=self.environment(NEURON_RELEASES_API=fake_release.unused_loopback_url()),
            capture_output=True,
            text=True,
            timeout=180,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"Installing Neuron {VERSION}", result.stdout)
        self.assertEqual(self.server.paths()[0], f"/v{VERSION}/SHA256SUMS")
        self.assertTrue(self.installed().is_file())

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

    def test_release_sums_that_disagree_or_are_missing_fail_closed(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        sums = self.releases / f"v{VERSION}" / "SHA256SUMS"
        sums.write_text(f"{'d' * 64}  {self.ASSET}\n", encoding="ascii")
        wrong = self.run_installer("-Version", VERSION)
        self.assertEqual(wrong.returncode, 1)
        self.assertIn("SHA256SUMS", wrong.stdout + wrong.stderr)
        self.assertFalse(self.installed().exists())
        sums.unlink()
        missing = self.run_installer("-Version", VERSION)
        self.assertEqual(missing.returncode, 1)
        self.assertFalse(self.installed().exists())

    def test_checksum_mismatch_installs_nothing(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET, checksum=f"{'b' * 64}  {self.ASSET}\n")
        result = self.run_installer("-Version", VERSION)
        self.assertEqual(result.returncode, 1)
        self.assertIn("checksum mismatch", result.stdout)
        self.assertFalse(self.installed().exists())

    def test_unpublished_version_and_insecure_base_fail_clearly(self) -> None:
        for downloader in ("curl", "dotnet"):
            with self.subTest(downloader=downloader):
                missing = self.run_installer("-Version", "9.9.9", NEURON_DOWNLOADER=downloader)
                self.assertEqual(missing.returncode, 1)
                self.assertIn("may not be published", missing.stdout)
        insecure = self.run_installer("-Version", VERSION, NEURON_DOWNLOAD_BASE="http://downloads.example.com")
        self.assertEqual(insecure.returncode, 1)
        self.assertIn("must be an HTTPS URL", insecure.stdout)

    def test_network_failure_is_not_reported_as_a_missing_release(self) -> None:
        for downloader in ("", "curl", "dotnet"):
            with self.subTest(downloader=downloader or "auto"):
                result = self.run_installer(
                    "-Version",
                    VERSION,
                    NEURON_DOWNLOAD_BASE=fake_release.unused_loopback_url(),
                    NEURON_CHECKSUMS_BASE=fake_release.unused_loopback_url(),
                    NEURON_DOWNLOADER=downloader,
                )
                self.assertEqual(result.returncode, 1)
                self.assertIn("Check your network", result.stdout)
                self.assertNotIn("may not be published", result.stdout)

    def test_the_ui_is_verified_by_checksum_and_never_started(self) -> None:
        # Starting a new unsigned program is when Windows Defender scans it, which can hold the
        # install up for minutes; the checksums are the check. These bytes cannot run at all.
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        fake_release.stage_release(self.releases, VERSION, "neuron-tui-windows-x64.exe", data=b"MZ not a program")
        result = self.run_installer("-Version", VERSION, NEURON_NO_TUI="")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("was not installed", result.stdout)
        self.assertEqual((self.install_dir / "neuron-tui.exe").read_bytes(), b"MZ not a program")

    def test_curl_that_cannot_start_falls_back_to_dotnet_once_for_the_whole_install(self) -> None:
        # As when AppLocker or WDAC blocks curl.exe: Process.Start throws. The first download
        # falls back to .NET with a message, and the later ones do not try curl.exe again.
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        body = f"""
$Version = '{VERSION}'
$InstallDir = ''
$AddToPath = $false
$NoPathChange = $true
function Get-NeuronCurl {{ return '{self.root / "blocked" / "curl.exe"}' }}
Install-Neuron
"""
        result = self.run_functions(self.INSTALL_FUNCTIONS, body, with_assignments=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count("curl.exe could not download"), 1, result.stdout)
        self.assertIn("curl.exe could not download SHA256SUMS (curl.exe could not start:", result.stdout)
        self.assertIn("using .NET for it and the remaining downloads", result.stdout)
        self.assertTrue(self.installed().is_file())
        self.assertEqual(
            self.server.paths(),
            [f"/v{VERSION}/SHA256SUMS", f"/v{VERSION}/{self.ASSET}.sha256", f"/v{VERSION}/{self.ASSET}"],
        )

    def test_curl_is_not_given_a_proxy_for_loopback_or_when_one_is_set(self) -> None:
        body = """
'loopback=[' + (Get-NeuronSystemProxy 'http://127.0.0.1:8080/x') + ']'
$env:HTTPS_PROXY = 'http://proxy.example:3128'
'env=[' + (Get-NeuronSystemProxy 'https://github.com/x') + ']'
"""
        result = self.run_functions(["Get-NeuronSystemProxy"], body)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["loopback=[]", "env=[]"])

    def test_reinstall_works_while_the_previous_upgrade_is_still_running(self) -> None:
        # neuron open in one terminal while the user upgrades twice from another: the running
        # image is renamed to neuron.exe.old and cannot be deleted until it exits.
        folder = self.root / "busy"
        folder.mkdir()
        destination = folder / "neuron.exe"
        system = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
        shutil.copyfile(system / "ping.exe", destination)
        running = subprocess.Popen(
            [str(destination), "-n", "60", "127.0.0.1"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        try:
            body = f"""
foreach ($round in 'first', 'second') {{
    Set-Content -LiteralPath '{folder / "download.exe"}' -Value $round -Encoding ascii
    try {{ Install-NeuronBinary '{folder / "download.exe"}' '{destination}'; "$round ok" }} catch {{ "$round error: $($_.Exception.Message)" }}
}}
"""
            result = self.run_functions(["Remove-StaleNeuronCopies", "Install-NeuronBinary"], body)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stdout.splitlines(), ["first ok", "second ok"])
            self.assertEqual(destination.read_text(encoding="ascii").strip(), "second")
            self.assertTrue(Path(f"{destination}.old").exists(), "the running copy stays until it exits")
        finally:
            running.kill()
            running.wait()
        # The next install clears what the running copy left behind.
        (folder / "download.exe").write_text("third", encoding="ascii")
        result = self.run_functions(
            ["Remove-StaleNeuronCopies", "Install-NeuronBinary"],
            f"Install-NeuronBinary '{folder / 'download.exe'}' '{destination}'",
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(sorted(entry.name for entry in folder.iterdir()), ["neuron.exe"])

    def test_the_full_path_command_survives_an_apostrophe_in_the_install_path(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        install_dir = self.root / "O'Brien" / "bin"
        quoted = str(install_dir).replace("'", "''")
        body = f"""
$Version = '{VERSION}'
$InstallDir = '{quoted}'
$AddToPath = $false
$NoPathChange = $true
Install-Neuron
"""
        result = self.run_functions(self.INSTALL_FUNCTIONS, body, with_assignments=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"& '{quoted}\\neuron.exe' login", result.stdout)
        self.assertTrue((install_dir / "neuron.exe").is_file())

    def test_release_lookup_accepts_a_list_or_a_single_release(self) -> None:
        body = """
'list=' + (Select-NeuronReleaseTag (ConvertFrom-Json '[{"tag_name":"v9.0.0","draft":true},{"tag_name":"v7.0.0-alpha.5","draft":false,"prerelease":true}]'))
'object=' + (Select-NeuronReleaseTag (ConvertFrom-Json '{"tag_name":"v6.3.0"}'))
'empty=' + (Select-NeuronReleaseTag (ConvertFrom-Json '[]'))
"""
        result = self.run_functions(["Select-NeuronReleaseTag"], body)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["list=v7.0.0-alpha.5", "object=v6.3.0", "empty="])

    def test_userinfo_in_the_download_base_is_rejected(self) -> None:
        for base in ("http://127.0.0.1:x@evil.example/", "https://user@mirror.example"):
            with self.subTest(base=base):
                result = self.run_installer("-Version", VERSION, NEURON_DOWNLOAD_BASE=base)
                self.assertEqual(result.returncode, 1)
                self.assertIn("must be an HTTPS URL", result.stdout)
        self.assertEqual(self.server.paths(), [])

    def test_github_token_is_not_sent_to_another_releases_api(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        result = self.run_installer(GITHUB_TOKEN=GITHUB_TOKEN)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.server.paths()[0], "/repos/neuron/releases/latest")
        self.assertEqual(self.server.authorization, [None, None, None, None])

    def test_github_token_is_meant_only_for_the_github_api(self) -> None:
        cases = {
            "https://api.github.com/repos/RAHUL-DevelopeRR/neuron-v2/releases/latest": "True",
            "https://api.github.com.evil.example/releases/latest": "False",
            "https://api.github.com@evil.example/releases/latest": "False",
            "http://api.github.com/releases/latest": "False",
            "https://mirror.example/releases/latest": "False",
            "not a url": "False",
        }
        body = "\n".join(f"'{url}=' + (Test-GitHubApiUrl '{url}')" for url in cases)
        result = self.run_functions(["Test-GitHubApiUrl"], body)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.splitlines(), [f"{url}={expected}" for url, expected in cases.items()])

    def test_reports_the_version_line_of_the_release_binary(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        result = self.run_installer("-Version", VERSION, NEURON_DUMMY_VERSION_REPORT="1")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        lines = result.stdout.splitlines()
        self.assertIn(f"  neuron {VERSION}", lines)
        self.assertNotIn("  Neuron", lines)

    def test_a_failed_replacement_restores_the_previous_binary(self) -> None:
        folder = self.root / "replace"
        folder.mkdir()
        destination, source = folder / "neuron.exe", folder / "download.exe"
        destination.write_text("previous", encoding="ascii")
        source.write_text("new", encoding="ascii")
        body = f"""
$script:moves = 0
function Move-Item {{
    param([string]$LiteralPath, [string]$Destination, [switch]$Force)
    $script:moves++
    # The second move puts the download in place; fail it as an antivirus lock would.
    if ($script:moves -eq 2) {{ throw 'The process cannot access the file because it is being used by another process.' }}
    Microsoft.PowerShell.Management\\Move-Item -LiteralPath $LiteralPath -Destination $Destination -Force:$Force
}}
try {{ Install-NeuronBinary '{source}' '{destination}'; 'no error' }} catch {{ "error: $($_.Exception.Message)" }}
"""
        result = self.run_functions(["Remove-StaleNeuronCopies", "Install-NeuronBinary"], body)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("error: could not install", result.stdout)
        self.assertIn("The previous version was kept.", result.stdout)
        self.assertEqual(destination.read_text(encoding="ascii"), "previous")
        self.assertFalse(Path(f"{destination}.old").exists())

    def run_functions(self, names: List[str], body: str, with_assignments: bool = False) -> subprocess.CompletedProcess:
        """Defines the named install.ps1 functions (from its AST) and runs `body` with them.

        `with_assignments` also runs the script's top-level assignments ($NeuronRepository, ...).
        """
        script = self.root / "functions.ps1"
        quoted = ", ".join(f"'{name}'" for name in names)
        script.write_text(
            f"""$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile('{INSTALL_PS1}', [ref]$null, [ref]$parseErrors)
if ({'$true' if with_assignments else '$false'}) {{
    foreach ($statement in $ast.EndBlock.Statements) {{
        if ($statement -is [System.Management.Automation.Language.AssignmentStatementAst]) {{
            . ([scriptblock]::Create($statement.Extent.Text))
        }}
    }}
}}
foreach ($name in @({quoted})) {{
    $definition = $ast.Find({{ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }}, $true)
    if (-not $definition) {{ throw "install.ps1 has no function $name" }}
    . ([scriptblock]::Create($definition.Extent.Text))
}}
{body}
""",
            encoding="utf-8",
        )
        return subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", str(script)],
            env=self.environment(),
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )

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

    def test_invoke_expression_install_leaves_the_session_preferences_alone(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        command = (
            "$ErrorActionPreference = 'Continue'; "
            f"Get-Content -Raw '{INSTALL_PS1}' | Invoke-Expression; "
            "\"preference=$ErrorActionPreference\"; "
            "try { $null -eq $neuronUndefinedVariable | Out-Null; 'strict=off' } catch { 'strict=on' }"
        )
        result = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", command],
            env=self.environment(NEURON_VERSION=VERSION),
            capture_output=True,
            text=True,
            timeout=180,
            check=False,
        )
        self.assertIn(f"Installing Neuron {VERSION}", result.stdout, result.stderr)
        self.assertTrue(self.installed().is_file(), result.stdout + result.stderr)
        self.assertIn(f"Neuron {VERSION} is installed.", result.stdout)
        self.assertNotIn("releases/latest", result.stdout)
        self.assertIn("preference=Continue", result.stdout)
        self.assertIn("strict=off", result.stdout)

    def test_posix_hint_names_the_requested_version(self) -> None:
        cases = {
            "7.0.0-alpha.1": "releases/download/v7.0.0-alpha.1/install.sh | sh -s -- --version 7.0.0-alpha.1",
            "v7.0.0-alpha.1": "releases/download/v7.0.0-alpha.1/install.sh | sh -s -- --version 7.0.0-alpha.1",
            "": "releases/download/<version>/install.sh | sh, with the newest version from https://github.com/RAHUL-DevelopeRR/neuron-v2/releases",
            "7.0; rm -rf ~": "releases/download/<version>/install.sh | sh, with the newest version from https://github.com/RAHUL-DevelopeRR/neuron-v2/releases",
        }
        body = "\n".join(f"'[' + (Get-PosixInstallCommand '{requested}') + ']'" for requested in cases)
        result = self.run_functions(["Get-PosixInstallCommand"], body, with_assignments=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(len(lines), len(cases), lines)
        for line, expected in zip(lines, cases.values()):
            self.assertEqual(line, f"[curl -fsSL https://github.com/RAHUL-DevelopeRR/neuron-v2/{expected}]")

    # Every function in install.ps1 except Add-UserPathEntry, which the tests stub.
    INSTALL_FUNCTIONS = [
        name
        for name in re.findall(r"(?m)^function ([A-Za-z0-9-]+)", INSTALL_PS1.read_text(encoding="utf-8"))
        if name != "Add-UserPathEntry"
    ]

    def run_add_to_path_install(self, path: str, no_path_change: bool = False) -> subprocess.CompletedProcess:
        """Runs Install-Neuron with the default PATH handling and PATH set to `path`, without writing the user PATH."""
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        body = f"""
$Version = '{VERSION}'
$InstallDir = ''
$AddToPath = $false
$NoPathChange = ${'true' if no_path_change else 'false'}
$env:NEURON_NO_MODIFY_PATH = $null
# Stands in for the registry write (test_add_to_path_updates_the_user_path_once covers that on CI).
function Add-UserPathEntry([string]$Directory) {{ Write-Host "user-path-stub $Directory"; return $true }}
$env:Path = '{path}'
Install-Neuron
$resolved = @(Get-Command neuron -CommandType Application -ErrorAction SilentlyContinue)
if ($resolved.Count -gt 0) {{ Write-Host "resolves-to $($resolved[0].Path)" }} else {{ Write-Host 'resolves-to nothing' }}
"""
        return self.run_functions(self.INSTALL_FUNCTIONS, body, with_assignments=True)

    def test_add_to_path_puts_this_install_before_an_older_neuron(self) -> None:
        # As on a PC with an old %LOCALAPPDATA%\Programs\NeuronCLI\neuron.exe on PATH (a v0.1.0
        # build that answers `neuron login` with "has been removed"): this install goes first, in
        # the user PATH and in this window, so `neuron` runs it straight away.
        shadow = self.root / "Programs" / "NeuronCLI"
        shadow.mkdir(parents=True)
        (shadow / "neuron.exe").write_bytes(b"MZ old neuron")
        system = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
        result = self.run_add_to_path_install(f"{shadow};{system}")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"user-path-stub {self.install_dir}", result.stdout)
        self.assertIn(f"put {self.install_dir} first in your user PATH", result.stdout)
        self.assertNotIn("comes first on PATH", result.stdout)
        self.assertIn(f"resolves-to {self.installed()}", result.stdout)
        self.assertNotIn("Open a new terminal window", result.stdout)
        self.assertTrue(self.installed().is_file())

    def test_shadow_warning_still_names_an_older_neuron_that_wins(self) -> None:
        # With -NoPathChange this install is not moved, so an older neuron already first still wins.
        shadow = self.root / "Programs" / "NeuronCLI"
        shadow.mkdir(parents=True)
        (shadow / "neuron.exe").write_bytes(b"MZ old neuron")
        result = self.run_add_to_path_install(f"{shadow};{self.install_dir}", no_path_change=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(
            f"another neuron at {shadow / 'neuron.exe'} comes first on PATH and runs instead of {self.installed()}",
            result.stdout,
        )
        self.assertIn(r'Remove-Item -Recurse -Force "$env:LOCALAPPDATA\Programs\NeuronCLI"', result.stdout)
        self.assertIn("npm.cmd uninstall -g @zero-x.live/neuron neuron-cli-app", result.stdout)

    def test_path_entry_first_moves_an_existing_entry(self) -> None:
        body = r"""
Write-Host "a: $(Get-PathWithEntryFirst 'C:\old;C:\N\bin\;C:\x' 'C:\N\bin')"
Write-Host "b: $(Get-PathWithEntryFirst '' 'C:\N\bin')"
Write-Host "c: $(Get-PathWithEntryFirst 'C:\N\bin;C:\x' 'C:\N\bin')"
"""
        result = self.run_functions(["Get-PathWithEntryFirst"], body)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(r"a: C:\N\bin;C:\old;C:\x", result.stdout)
        self.assertIn(r"b: C:\N\bin", result.stdout)
        self.assertIn(r"c: C:\N\bin;C:\x", result.stdout)

    def test_add_to_path_without_another_neuron_does_not_warn(self) -> None:
        system = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
        result = self.run_add_to_path_install(str(system))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"put {self.install_dir} first in your user PATH", result.stdout)
        self.assertNotIn("comes first on PATH", result.stdout)
        self.assertNotIn("Open a new terminal window", result.stdout, "irm | iex: neuron works in this window")
        self.assertIn(f"resolves-to {self.installed()}", result.stdout)
        self.assertIn("Sign in:  neuron login", result.stdout)

    def test_no_path_change_leaves_the_user_path_alone(self) -> None:
        system = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"
        result = self.run_add_to_path_install(str(system), no_path_change=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("user-path-stub", result.stdout)
        self.assertIn("is not on your PATH", result.stdout)
        self.assertIn(f"& '{self.installed()}' login", result.stdout)

    def test_core_is_installed_before_the_ui_download_starts(self) -> None:
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        fake_release.stage_release(self.releases, VERSION, "neuron-tui-windows-x64.exe")
        result = self.run_installer("-Version", VERSION, NEURON_NO_TUI="")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        output = result.stdout
        self.assertLess(output.index(f"Installed {self.installed()}"), output.index("neuron-tui.exe: "))
        self.assertLess(output.index("neuron is ready"), output.index("neuron-tui.exe: "))

    def test_installs_nothing_that_needs_script_execution(self) -> None:
        # Under the default "Restricted" policy PowerShell runs .exe files but no .ps1 shim.
        fake_release.stage_release(self.releases, VERSION, self.ASSET)
        fake_release.stage_release(self.releases, VERSION, "neuron-tui-windows-x64.exe")
        result = self.run_installer("-Version", VERSION, NEURON_NO_TUI="")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(sorted(entry.name for entry in self.install_dir.iterdir()), ["neuron-tui.exe", "neuron.exe"])

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
            self.assertEqual(entries[0], str(self.install_dir).lower(), "this install comes first in the user PATH")
        finally:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment", 0, winreg.KEY_SET_VALUE) as key:
                if original is None:
                    winreg.DeleteValue(key, "Path")
                else:
                    winreg.SetValueEx(key, "Path", 0, original[1], original[0])


if __name__ == "__main__":
    unittest.main()
