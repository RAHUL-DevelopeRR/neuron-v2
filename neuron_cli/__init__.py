"""NeuronCLI - Python shim for the Rust binary."""

from __future__ import annotations

import hashlib
import importlib.metadata
import os
import platform
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.request import urlopen


PACKAGE_VERSION = "6.2.5"
REPOSITORY = "https://github.com/RAHUL-DevelopeRR/neuron-v2"


def _package_version() -> str:
    try:
        return importlib.metadata.version("neuroncli")
    except importlib.metadata.PackageNotFoundError:
        return PACKAGE_VERSION


def _release_asset() -> str:
    system, machine = platform.system(), platform.machine().lower()
    if system == "Windows" and machine in {"amd64", "x86_64"}:
        return "neuron-windows-x64.exe"
    if system == "Linux" and machine in {"amd64", "x86_64"}:
        return "neuron-linux-x64"
    if system == "Darwin" and machine in {"arm64", "aarch64"}:
        return "neuron-macos-arm64"
    raise RuntimeError(f"NeuronCLI does not yet publish a binary for {system} {machine}.")


def _cache_dir(version: str) -> Path:
    if sys.platform == "win32":
        root = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData/Local"))
    elif sys.platform == "darwin":
        root = Path.home() / "Library/Caches"
    else:
        root = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    return root / "neuroncli" / version


def _digest(path: Path) -> str:
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def _download_binary() -> Path:
    asset = _release_asset()
    version = _package_version()
    base = f"{REPOSITORY}/releases/download/v{version}"
    cache = _cache_dir(version)
    destination = cache / asset
    cache.mkdir(parents=True, exist_ok=True)

    checksum_url = f"{base}/{asset}.sha256"
    with urlopen(checksum_url, timeout=30) as response:
        expected = response.read(512).decode("ascii").split()[0].lower()
    if len(expected) != 64 or any(char not in "0123456789abcdef" for char in expected):
        raise RuntimeError(f"Invalid SHA-256 file for NeuronCLI {version}.")
    if destination.is_file() and _digest(destination) == expected:
        return destination

    temporary_path = None
    try:
        with urlopen(f"{base}/{asset}", timeout=60) as response, tempfile.NamedTemporaryFile(
            dir=cache, prefix=".neuron-", delete=False
        ) as temporary:
            temporary_path = Path(temporary.name)
            checksum = hashlib.sha256()
            while chunk := response.read(1024 * 1024):
                temporary.write(chunk)
                checksum.update(chunk)
        if checksum.hexdigest() != expected:
            raise RuntimeError(f"NeuronCLI {version} binary failed its SHA-256 check.")
        if os.name != "nt":
            temporary_path.chmod(0o755)
        temporary_path.replace(destination)
        return destination
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)


def _find_binary() -> Path:
    """Locate this package's binary, with an explicit development override."""
    override = os.environ.get("NEURON_BINARY_PATH")
    if override:
        candidate = Path(override).expanduser().resolve()
        if not candidate.is_file() or candidate == Path(sys.argv[0]).resolve():
            raise RuntimeError("NEURON_BINARY_PATH must point to a native Neuron binary.")
        return candidate
    pkg_dir = Path(__file__).resolve().parent
    binary_name = "neuron.exe" if sys.platform == "win32" else "neuron"
    candidate = pkg_dir / binary_name
    if candidate.is_file():
        return candidate
    return _download_binary()


def main() -> None:
    """Invoke the Rust neuron binary with forwarded argv."""
    binary = _find_binary()
    # Replace sys.argv[0] with the actual binary path so the Rust CLI
    # sees correct program name in --version / help text.
    args = [str(binary), *sys.argv[1:]]
    # os.execv on Windows does not reliably inherit console std streams,
    # which silently swallows output for --version / --help.  Use
    # subprocess.run instead and forward the child's exit code.
    result = subprocess.run(args, check=False)
    sys.exit(result.returncode)


if __name__ == "__main__":
    main()
