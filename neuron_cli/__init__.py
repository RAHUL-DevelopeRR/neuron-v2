"""NeuronCLI - Python shim for the bundled native binary."""

from __future__ import annotations

import os
import platform as _platform
import subprocess
import sys
from pathlib import Path


def _find_binary() -> Path:
    """Locate the embedded neuron binary inside the installed package."""
    pkg_dir = Path(__file__).resolve().parent
    binary_name = "neuron.exe" if sys.platform == "win32" else "neuron"

    platform_name = {
        "win32": "windows",
        "darwin": "darwin",
        "linux": "linux",
    }.get(sys.platform, sys.platform)
    machine = _platform.machine().lower()
    arch = {
        "x86_64": "amd64",
        "amd64": "amd64",
        "aarch64": "arm64",
        "arm64": "arm64",
    }.get(machine, machine)
    ext = ".exe" if sys.platform == "win32" else ""

    candidates = [
        pkg_dir / "bin" / f"neuron-{platform_name}-{arch}{ext}",
        pkg_dir / binary_name,
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    # Fallback: search PATH (useful during development)
    for path_dir in os.get_exec_path():
        candidate = Path(path_dir) / binary_name
        if candidate.exists():
            return candidate
    raise RuntimeError(
        f"NeuronCLI binary '{binary_name}' not found inside package or PATH. "
        "Try reinstalling: pip install --force-reinstall neuroncli"
    )


def _ensure_scripts_on_path() -> None:
    """Ensure pip's Scripts directory is on PATH.

    On Windows, `pip install` puts the `neuron` entry-point into
    `<python>/Scripts/` but that directory is often NOT on the user's
    PATH, so `neuron` fails with "command not found" after install.

    This function detects the situation and patches the user's PATH
    for the current process (and optionally persists it).
    """
    if sys.platform != "win32":
        return

    scripts_dir = Path(sys.executable).parent / "Scripts"
    if not scripts_dir.exists():
        return

    scripts_str = str(scripts_dir)
    current_path = os.environ.get("PATH", "")
    if scripts_str.lower() in current_path.lower():
        return  # Already on PATH

    # Patch for current process
    os.environ["PATH"] = scripts_str + os.pathsep + current_path

    # Try to persist to user PATH via setx (best-effort, silent fail)
    try:
        # Read current user PATH from registry
        import winreg
        with winreg.OpenKey(
            winreg.HKEY_CURRENT_USER,
            r"Environment",
            0,
            winreg.KEY_READ,
        ) as key:
            try:
                user_path, _ = winreg.QueryValueEx(key, "Path")
            except FileNotFoundError:
                user_path = ""

        if scripts_str.lower() not in user_path.lower():
            new_path = scripts_str + os.pathsep + user_path if user_path else scripts_str
            subprocess.run(
                ["setx", "PATH", new_path],
                capture_output=True,
                check=False,
            )
            print(
                f"\033[32m✓\033[0m Added {scripts_str} to user PATH. "
                "Restart your terminal for the change to take effect.",
                file=sys.stderr,
            )
    except Exception:
        pass  # Non-critical — PATH is patched for current process at minimum


def main() -> None:
    """Invoke the native neuron binary with forwarded argv."""
    _ensure_scripts_on_path()
    binary = _find_binary()
    # Replace sys.argv[0] with the actual binary path so the native CLI
    # sees correct program name in --version / help text.
    args = [str(binary), *sys.argv[1:]]
    # os.execv on Windows does not reliably inherit console std streams,
    # which silently swallows output for --version / --help.  Use
    # subprocess.run instead and forward the child's exit code.
    result = subprocess.run(args, check=False)
    sys.exit(result.returncode)


if __name__ == "__main__":
    main()
