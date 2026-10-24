"""NeuronCLI - Python launcher for the native Neuron binary.

The first launch downloads the release binary that matches this package
version, verifies it against the release's SHA-256 file and caches it together
with a ``.sha256`` sidecar. Later launches run the cached binary without any
network access.

On a terminal, a bare ``neuron`` opens Neuron's full-screen UI: the launcher
gets ``neuron-tui`` the same way (once, verified and cached) and tells
``neuron`` where it is (``NEURON_TUI_BIN``). If it can't, ``neuron`` runs its
line REPL as before.

Environment:
    NEURON_BINARY_PATH     run this native binary instead (development)
    NEURON_CACHE_DIR       cache root (default: the platform user cache)
    NEURON_DOWNLOAD_BASE   release download base URL (HTTPS, or loopback HTTP)
    NEURON_NO_TUI          1 keeps the line REPL and skips the UI download
    NEURON_TUI_BIN         the full-screen UI to use (passed on unchanged)
"""

from __future__ import annotations

import hashlib
import ipaddress
import os
import platform
import signal
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable, Dict, List, Optional
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import urlopen

PACKAGE_VERSION = "7.0.0-alpha.1"
REPOSITORY = "https://github.com/RAHUL-DevelopeRR/neuron-v2"
DEFAULT_DOWNLOAD_BASE = f"{REPOSITORY}/releases/download"
CHECKSUM_TIMEOUT_SECONDS = 30
BINARY_TIMEOUT_SECONDS = 120


class LauncherError(Exception):
    """A failure with a message meant for the user (no traceback)."""


def _package_version() -> str:
    """The release this launcher runs, spelled like its tag (``v<version>``).

    Not importlib.metadata: it returns the PEP 440 normal form (``6.4.0rc1``),
    while tags and asset URLs use the semver spelling (``v6.4.0-rc.1``). The
    release workflow and tests/test_packaging_metadata.py keep PACKAGE_VERSION
    equal to the version in pyproject.toml.
    """
    return PACKAGE_VERSION


def _release_asset(
    system: Optional[str] = None, machine: Optional[str] = None, component: str = "neuron"
) -> str:
    """The release asset for this platform: the CLI's, or with ``component="ui"``
    the full-screen UI's (``neuron-tui-...`` beside each ``neuron-...``)."""
    asset = _cli_release_asset(system, machine)
    return "neuron-tui-" + asset[len("neuron-"):] if component == "ui" else asset


def _cli_release_asset(system: Optional[str] = None, machine: Optional[str] = None) -> str:
    system = system or platform.system()
    machine = (machine or platform.machine()).lower()
    if system == "Windows" and machine in {"amd64", "x86_64", "arm64", "aarch64"}:
        # Windows 11 on ARM runs the x64 build under emulation.
        return "neuron-windows-x64.exe"
    if system == "Linux" and machine in {"amd64", "x86_64"}:
        return "neuron-linux-x64"
    if system == "Linux" and machine in {"arm64", "aarch64"}:
        return "neuron-linux-arm64"
    if system == "Darwin" and machine in {"arm64", "aarch64"}:
        return "neuron-macos-arm64"
    if system == "Darwin" and machine in {"amd64", "x86_64"}:
        return "neuron-macos-x64"
    raise LauncherError(
        f"Neuron does not publish a binary for {system} {machine}. Supported: Linux x64/arm64, "
        f"macOS arm64/x64 and Windows x64. Build from source ({REPOSITORY}) and set "
        "NEURON_BINARY_PATH to the built binary."
    )


def _cache_root() -> Path:
    override = os.environ.get("NEURON_CACHE_DIR")
    if override:
        return Path(override).expanduser()
    if sys.platform == "win32":
        return Path(os.environ.get("LOCALAPPDATA") or Path.home() / "AppData" / "Local") / "neuroncli"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Caches" / "neuroncli"
    return Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "neuroncli"


def _cache_dir(version: str) -> Path:
    return _cache_root() / version


def _is_loopback(host: str) -> bool:
    if host == "localhost":
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


def _download_base() -> str:
    base = (os.environ.get("NEURON_DOWNLOAD_BASE") or DEFAULT_DOWNLOAD_BASE).rstrip("/")
    parts = urlsplit(base)
    if parts.scheme == "https" or (parts.scheme == "http" and _is_loopback(parts.hostname or "")):
        return base
    raise LauncherError(f"NEURON_DOWNLOAD_BASE must be an HTTPS URL (or loopback HTTP): {base}")


def _digest(path: Path) -> str:
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def _parse_checksum(text: str) -> Optional[str]:
    fields = text.split()
    if not fields:
        return None
    candidate = fields[0].lower()
    if len(candidate) != 64 or any(char not in "0123456789abcdef" for char in candidate):
        return None
    return candidate


def _sidecar(binary: Path) -> Path:
    return binary.with_name(binary.name + ".sha256")


def _cached_binary(cache: Path, asset: str) -> Optional[Path]:
    """The cached binary when it was verified at download time (sidecar present)."""
    binary = cache / asset
    sidecar = _sidecar(binary)
    if not binary.is_file() or not sidecar.is_file():
        return None
    try:
        if _parse_checksum(sidecar.read_text(encoding="ascii", errors="replace")) is None:
            return None
    except OSError:
        return None
    return binary


def _network_error(url: str, version: str, cache: Path, error: BaseException) -> LauncherError:
    if isinstance(error, HTTPError):
        if error.code == 404:
            return LauncherError(
                f"Neuron {version} has no release asset at {url} (HTTP 404). The release may not be "
                f"published yet; see {REPOSITORY}/releases, or upgrade with: pip install --upgrade neuroncli"
            )
        return LauncherError(f"Downloading {url} failed with HTTP {error.code} {error.reason}. Try again later.")
    reason = getattr(error, "reason", None) or error
    return LauncherError(
        f"Could not download Neuron {version} from {url}: {reason}. The first launch needs access to "
        f"GitHub Releases; later launches run offline from {cache}. Check your network or proxy "
        "(HTTPS_PROXY), or set NEURON_BINARY_PATH to a local neuron binary."
    )


def _replace_atomically(cache: Path, destination: Path, write: Callable[[Path], None]) -> None:
    descriptor, name = tempfile.mkstemp(dir=cache, prefix=".neuron-")
    os.close(descriptor)
    temporary = Path(name)
    try:
        write(temporary)
        os.replace(temporary, destination)
    finally:
        if temporary.exists():
            temporary.unlink()


def _download_binary(version: str, asset: str, cache: Path, label: str = "Neuron") -> Path:
    base = f"{_download_base()}/v{version}"
    binary_url = f"{base}/{asset}"
    checksum_url = f"{binary_url}.sha256"
    destination = cache / asset
    try:
        cache.mkdir(parents=True, exist_ok=True)
    except OSError as error:
        raise LauncherError(f"Cannot create the Neuron cache directory {cache}: {error}. Set NEURON_CACHE_DIR.") from error

    print(f"Downloading {label} {version} ({asset}) to {cache} ...", file=sys.stderr, flush=True)
    try:
        with urlopen(checksum_url, timeout=CHECKSUM_TIMEOUT_SECONDS) as response:
            checksum_text = response.read(4096).decode("ascii", errors="replace")
    except (HTTPError, URLError, OSError) as error:
        raise _network_error(checksum_url, version, cache, error) from error
    expected = _parse_checksum(checksum_text)
    if expected is None:
        raise LauncherError(f"{checksum_url} does not contain a SHA-256 checksum.")

    def write_binary(temporary: Path) -> None:
        checksum = hashlib.sha256()
        with temporary.open("wb") as stream:
            try:
                response = urlopen(binary_url, timeout=BINARY_TIMEOUT_SECONDS)
            except (HTTPError, URLError, OSError) as error:
                raise _network_error(binary_url, version, cache, error) from error
            with response:
                while True:
                    try:
                        chunk = response.read(1024 * 1024)
                    except OSError as error:
                        raise _network_error(binary_url, version, cache, error) from error
                    if not chunk:
                        break
                    stream.write(chunk)
                    checksum.update(chunk)
        actual = checksum.hexdigest()
        if actual != expected:
            raise LauncherError(
                f"Neuron {version} ({asset}) failed its SHA-256 check: expected {expected}, got {actual}. "
                "The download was discarded; nothing was cached."
            )
        if os.name != "nt":
            temporary.chmod(0o755)

    try:
        _replace_atomically(cache, destination, write_binary)
        _replace_atomically(
            cache,
            _sidecar(destination),
            lambda temporary: temporary.write_text(f"{expected}  {asset}\n", encoding="ascii"),
        )
    except OSError as error:
        raise LauncherError(
            f"Cannot write Neuron to {cache}: {error}. Free some disk space or set NEURON_CACHE_DIR."
        ) from error
    return destination


def _find_binary() -> Path:
    """Locate the native binary: override, bundled, verified cache, then download."""
    override = os.environ.get("NEURON_BINARY_PATH")
    if override:
        candidate = Path(override).expanduser().resolve()
        if not candidate.is_file() or candidate == Path(sys.argv[0]).resolve():
            raise LauncherError("NEURON_BINARY_PATH must point to a native Neuron binary.")
        return candidate
    bundled = Path(__file__).resolve().parent / ("neuron.exe" if sys.platform == "win32" else "neuron")
    if bundled.is_file():
        return bundled
    version = _package_version()
    asset = _release_asset()
    cache = _cache_dir(version)
    cached = _cached_binary(cache, asset)
    if cached is not None:
        return cached
    return _download_binary(version, asset, cache)


# The flags whose value is the next argument (args.rs VALUE_FLAGS, plus --provider).
_VALUE_FLAGS = {
    "--model",
    "--provider",
    "--output-format",
    "--permission-mode",
    "--base-commit",
    "--reasoning-effort",
    "--allowedTools",
    "--allowed-tools",
}


def _truthy(value: Optional[str]) -> bool:
    return (value or "").strip().lower() not in {"", "0", "false", "no"}


def _wants_full_screen_ui(args: List[str], interactive: bool, env: Optional[Dict[str, str]] = None) -> bool:
    """Whether ``neuron <args>`` will open the full-screen UI: on a terminal, not
    turned off (``--no-tui``, NEURON_NO_TUI), and only options, no prompt or
    subcommand. ``neuron`` makes the final call; this avoids fetching the UI
    for runs that won't use it."""
    env = os.environ if env is None else env
    if not interactive or _truthy(env.get("NEURON_NO_TUI")):
        return False
    index = 0
    while index < len(args):
        arg = args[index]
        if arg in {"--no-tui", "--help", "-h", "--version", "-V"}:
            return False
        if arg in _VALUE_FLAGS:
            index += 2
            continue
        if arg.startswith("-") and arg not in {"-", "-p", "--"}:
            index += 1
            continue
        return False
    return True


def _find_ui_binary(args: List[str]) -> Optional[Path]:
    """The full-screen UI for this run, or None: not wanted, or not available
    (then ``neuron`` runs its line REPL). A UI the user points at with
    NEURON_TUI_BIN is left to ``neuron``."""
    if os.environ.get("NEURON_TUI_BIN"):
        return None
    interactive = sys.stdin.isatty() and sys.stdout.isatty()
    if not _wants_full_screen_ui(args, interactive):
        return None
    bundled = Path(__file__).resolve().parent / ("neuron-tui.exe" if sys.platform == "win32" else "neuron-tui")
    if bundled.is_file():
        return bundled
    try:
        version = _package_version()
        asset = _release_asset(component="ui")
        cache = _cache_dir(version)
        cached = _cached_binary(cache, asset)
        if cached is not None:
            return cached
        return _download_binary(version, asset, cache, label="Neuron's full-screen UI")
    except LauncherError as error:
        print(f"neuron: the full-screen UI is not available ({error}); using the line REPL.", file=sys.stderr)
        return None


def _ignore_signal(_signum: int, _frame: object) -> None:
    """Parent-side handler: the child receives terminal signals itself."""


def _run_child(args: List[str]) -> int:
    """Run the binary as a child, keeping the parent alive through Ctrl+C.

    On Windows a console Ctrl+C or Ctrl+Break reaches the child directly; the
    parent must ignore it so it does not exit (or print a traceback) while the
    child keeps running. On POSIX a Python-level handler is used instead of
    SIG_IGN because ignored signals would be inherited by the child.
    """
    handler = signal.SIG_IGN if os.name == "nt" else _ignore_signal
    previous: Dict[int, object] = {}
    for name in ("SIGINT", "SIGBREAK"):
        signum = getattr(signal, name, None)
        if signum is not None:
            previous[signum] = signal.signal(signum, handler)
    try:
        with subprocess.Popen(args) as process:
            return process.wait()
    finally:
        for signum, old in previous.items():
            signal.signal(signum, old)


def _exit_status(returncode: int) -> int:
    """Translate a child return code into this process's exit status."""
    if returncode < 0:
        return 128 + (-returncode)  # POSIX: terminated by a signal
    if os.name == "nt" and returncode > 0x7FFFFFFF:
        return returncode - 0x1_0000_0000  # NTSTATUS codes, e.g. 0xC000013A
    return returncode


def main() -> None:
    """Run the native neuron binary with this process's arguments."""
    try:
        binary = _find_binary()
    except LauncherError as error:
        print(f"neuron: {error}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("neuron: download cancelled.", file=sys.stderr)
        sys.exit(130)

    try:
        ui = _find_ui_binary(sys.argv[1:])
    except KeyboardInterrupt:
        print("neuron: download cancelled; using the line REPL.", file=sys.stderr)
        ui = None
    if ui is not None:
        os.environ["NEURON_TUI_BIN"] = str(ui)

    args = [str(binary), *sys.argv[1:]]
    if os.name == "posix":
        sys.stdout.flush()
        sys.stderr.flush()
        try:
            os.execv(args[0], args)
        except OSError as error:
            print(f"neuron: could not start {binary}: {error}", file=sys.stderr)
            sys.exit(1)
    try:
        returncode = _run_child(args)
    except OSError as error:
        print(f"neuron: could not start {binary}: {error}", file=sys.stderr)
        sys.exit(1)
    sys.exit(_exit_status(returncode))


if __name__ == "__main__":
    main()
