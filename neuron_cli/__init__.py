"""NeuronCLI Python launcher for the native Go binary."""

from __future__ import annotations

import hashlib
import importlib.metadata
import os
import platform as _platform
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path


RELEASE_REPOSITORY = os.environ.get("NEURON_RELEASE_REPOSITORY", "RAHUL-DevelopeRR/neuron-v2")


def _version() -> str:
    try:
        return importlib.metadata.version("neuroncli")
    except importlib.metadata.PackageNotFoundError:
        return "6.3.0"


def _platform_name() -> str:
    return {
        "win32": "windows",
        "darwin": "darwin",
        "linux": "linux",
    }.get(sys.platform, sys.platform)


def _arch() -> str:
    machine = _platform.machine().lower()
    return {
        "x86_64": "amd64",
        "amd64": "amd64",
        "aarch64": "arm64",
        "arm64": "arm64",
    }.get(machine, machine)


def _binary_name() -> str:
    return "neuron.exe" if sys.platform == "win32" else "neuron"


def _release_binary_name() -> str:
    ext = ".exe" if sys.platform == "win32" else ""
    return f"neuron-{_platform_name()}-{_arch()}{ext}"


def _release_url(asset: str) -> str:
    return f"https://github.com/{RELEASE_REPOSITORY}/releases/download/v{_version()}/{asset}"


def _cache_dir() -> Path:
    if sys.platform == "win32":
        base = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local"))
    else:
        base = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    return base / "neuroncli" / "bin" / _version() / f"{sys.platform}-{_arch()}"


def _download(url: str, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(url, timeout=30) as response:
        with target.open("wb") as file:
            shutil.copyfileobj(response, file)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _verify_checksum(asset: str, path: Path) -> None:
    checksum_file = _cache_dir() / "checksums.txt"
    _download(_release_url("checksums.txt"), checksum_file)
    expected = ""
    for line in checksum_file.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[-1] == asset:
            expected = parts[0].lower()
            break
    if not expected:
        raise RuntimeError(f"checksum entry for {asset} not found")
    actual = _sha256(path)
    if actual != expected:
        raise RuntimeError(f"checksum mismatch for {asset}")


def _download_release_binary() -> Path | None:
    if os.environ.get("NEURON_DISABLE_DOWNLOAD", "").lower() in {"1", "true", "yes"}:
        return None
    target = _cache_dir() / _binary_name()
    if target.exists():
        return target
    asset = _release_binary_name()
    tmp = target.with_suffix(target.suffix + ".download")
    try:
        _download(_release_url(asset), tmp)
        _verify_checksum(asset, tmp)
        tmp.replace(target)
        if sys.platform != "win32":
            target.chmod(0o755)
        return target
    except (urllib.error.URLError, RuntimeError) as err:
        raise RuntimeError(f"failed to download {asset}: {err}") from err


def _find_binary() -> Path:
    pkg_dir = Path(__file__).resolve().parent
    candidates = [
        pkg_dir / "bin" / _binary_name(),
        pkg_dir / "bin" / _release_binary_name(),
        pkg_dir / _binary_name(),
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    for path_dir in os.get_exec_path():
        candidate = Path(path_dir) / _binary_name()
        if candidate.exists():
            return candidate
    downloaded = _download_release_binary()
    if downloaded and downloaded.exists():
        return downloaded
    raise RuntimeError(
        f"NeuronCLI binary '{_binary_name()}' was not found for {sys.platform}/{_arch()}. "
        "Try reinstalling with: pipx install neuroncli"
    )


def _ensure_scripts_on_path() -> None:
    if sys.platform != "win32":
        return
    scripts_dir = Path(sys.executable).parent / "Scripts"
    if scripts_dir.exists() and str(scripts_dir).lower() not in os.environ.get("PATH", "").lower():
        os.environ["PATH"] = str(scripts_dir) + os.pathsep + os.environ.get("PATH", "")


def main() -> None:
    _ensure_scripts_on_path()
    binary = _find_binary()
    env = os.environ.copy()
    env.setdefault("NEURON_INSTALL_SOURCE", "pypi")
    result = subprocess.run([str(binary), *sys.argv[1:]], check=False, env=env)
    sys.exit(result.returncode)


if __name__ == "__main__":
    main()
