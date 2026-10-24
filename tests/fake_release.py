"""Local fake GitHub release server and dummy Neuron binaries for packaging tests.

Used by the unit tests and by .github/workflows/packaging-ci.yml:

    python tests/fake_release.py stage --root DIR --version 6.3.0 [--asset NAME]
    python tests/fake_release.py serve --root DIR --port-file FILE

``stage`` writes ``DIR/v<version>/<asset>`` (a dummy binary for this machine)
and ``<asset>.sha256`` in the release format ``<sha256>  <asset>``. ``serve``
serves DIR over HTTP on 127.0.0.1 until it is killed, answering
``/releases/latest`` with ``{"tag_name": "v<newest staged version>"}``.

The dummy binary prints ``neuron-dummy <version>`` for ``--version`` (or the real
binary's ``NeuronCLI`` / ``Version`` report when ``$NEURON_DUMMY_VERSION_REPORT`` is
set) and its arguments otherwise, prints ``pid=<pid>`` when ``$NEURON_DUMMY_PID`` is
set (POSIX), and exits with ``$NEURON_DUMMY_EXIT`` (default 0).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import stat
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Dict, List, Optional, Tuple

POSIX_DUMMY = """#!/bin/sh
if [ "${{1:-}}" = "--version" ] && [ -n "${{NEURON_DUMMY_VERSION_REPORT:-}}" ]; then
    echo "NeuronCLI"
    echo "  Version          {version}"
    echo "  Build date       2026-01-01"
elif [ "${{1:-}}" = "--version" ]; then
    echo "neuron-dummy {version}"
else
    echo "neuron-dummy args: $*"
fi
if [ -n "${{NEURON_DUMMY_PID:-}}" ]; then
    echo "pid=$$"
fi
exit "${{NEURON_DUMMY_EXIT:-0}}"
"""

WINDOWS_DUMMY = """using System;
class NeuronDummy {{
    static int Main(string[] args) {{
        if (args.Length > 0 && args[0] == "--version" && !string.IsNullOrEmpty(Environment.GetEnvironmentVariable("NEURON_DUMMY_VERSION_REPORT"))) {{
            Console.WriteLine("NeuronCLI");
            Console.WriteLine("  Version          {version}");
            Console.WriteLine("  Build date       2026-01-01");
        }} else if (args.Length > 0 && args[0] == "--version") {{
            Console.WriteLine("neuron-dummy {version}");
        }} else {{
            Console.WriteLine("neuron-dummy args: " + string.Join(" ", args));
        }}
        int code;
        return int.TryParse(Environment.GetEnvironmentVariable("NEURON_DUMMY_EXIT"), out code) ? code : 0;
    }}
}}
"""

__all__ = [
    "FakeReleaseServer",
    "can_build_dummy",
    "checksum_line",
    "current_asset",
    "dummy_binary_bytes",
    "stage_release",
    "unused_loopback_url",
    "write_executable",
]

_compiled: Dict[str, bytes] = {}


def current_asset() -> str:
    """Release asset name for this machine, mirroring the installers."""
    system, machine = platform.system(), platform.machine().lower()
    arch = {"x86_64": "x64", "amd64": "x64", "aarch64": "arm64", "arm64": "arm64"}.get(machine)
    if system == "Windows":
        return "neuron-windows-x64.exe"
    if system == "Linux" and arch:
        return f"neuron-linux-{arch}"
    if system == "Darwin" and arch:
        return f"neuron-macos-{arch}"
    raise RuntimeError(f"no Neuron release asset for {system} {machine}")


def csc_path() -> Optional[Path]:
    """The .NET Framework C# compiler that ships with Windows, if present."""
    windir = Path(os.environ.get("WINDIR", r"C:\Windows"))
    for framework in ("Framework64", "Framework"):
        candidate = windir / "Microsoft.NET" / framework / "v4.0.30319" / "csc.exe"
        if candidate.is_file():
            return candidate
    return None


def can_build_dummy() -> bool:
    return os.name != "nt" or csc_path() is not None


def dummy_binary_bytes(version: str) -> bytes:
    """Bytes of a runnable dummy Neuron binary for this machine."""
    if os.name != "nt":
        return POSIX_DUMMY.format(version=version).encode()
    if version in _compiled:
        return _compiled[version]
    compiler = csc_path()
    if compiler is None:
        raise RuntimeError("csc.exe (.NET Framework 4) is needed to build the Windows dummy binary")
    with tempfile.TemporaryDirectory() as temporary:
        source = Path(temporary) / "dummy.cs"
        output = Path(temporary) / "dummy.exe"
        source.write_text(WINDOWS_DUMMY.format(version=version), encoding="utf-8")
        subprocess.run(
            [str(compiler), "/nologo", "/target:exe", f"/out:{output}", str(source)],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        _compiled[version] = output.read_bytes()
    return _compiled[version]


def write_executable(path: Path, data: bytes) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


def checksum_line(data: bytes, asset: str) -> str:
    return f"{hashlib.sha256(data).hexdigest()}  {asset}\n"


def stage_release(
    root: Path,
    version: str,
    asset: Optional[str] = None,
    data: Optional[bytes] = None,
    checksum: Optional[str] = None,
) -> Path:
    """Write a release asset and its .sha256 under ``root/v<version>/``."""
    asset = asset or current_asset()
    data = dummy_binary_bytes(version) if data is None else data
    directory = root / f"v{version}"
    write_executable(directory / asset, data)
    sidecar = checksum if checksum is not None else checksum_line(data, asset)
    (directory / f"{asset}.sha256").write_text(sidecar, encoding="ascii")
    return directory / asset


class FakeReleaseServer:
    """Serves a release directory on 127.0.0.1 and records every request."""

    def __init__(self, root: Path, latest_tag: Optional[str] = None) -> None:
        self.root = Path(root)
        self.latest_tag = latest_tag
        self.requests: List[Tuple[str, str]] = []
        self.authorization: List[Optional[str]] = []  # Authorization header of each request
        self._lock = threading.Lock()
        server = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args) -> None:  # keep test output quiet
                pass

            def _respond(self, include_body: bool) -> None:
                path = self.path.split("?", 1)[0]
                with server._lock:
                    server.requests.append((self.command, path))
                    server.authorization.append(self.headers.get("Authorization"))
                if path.endswith("/releases/latest"):
                    tag = server.latest_tag or server.newest_tag()
                    if tag is None:
                        self._send(404, b'{"message": "Not Found"}', "application/json", include_body)
                    else:
                        body = json.dumps({"tag_name": tag, "name": tag}).encode()
                        self._send(200, body, "application/json", include_body)
                    return
                target = (server.root / path.lstrip("/")).resolve()
                if server.root.resolve() not in target.parents or not target.is_file():
                    self._send(404, b"Not Found", "text/plain", include_body)
                    return
                self._send(200, target.read_bytes(), "application/octet-stream", include_body)

            def _send(self, status: int, body: bytes, content_type: str, include_body: bool) -> None:
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                if include_body:
                    self.wfile.write(body)

            def do_GET(self) -> None:  # noqa: N802 - http.server API
                self._respond(include_body=True)

            def do_HEAD(self) -> None:  # noqa: N802 - http.server API
                self._respond(include_body=False)

        self._httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._thread: Optional[threading.Thread] = None

    @property
    def port(self) -> int:
        return self._httpd.server_address[1]

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def newest_tag(self) -> Optional[str]:
        versions = [entry.name for entry in self.root.glob("v*") if entry.is_dir()]
        if not versions:
            return None
        return max(versions, key=lambda name: [int(part) if part.isdigit() else 0 for part in name[1:].split(".")])

    def paths(self) -> List[str]:
        with self._lock:
            return [path for _method, path in self.requests]

    def start(self) -> "FakeReleaseServer":
        self._thread = threading.Thread(target=self._httpd.serve_forever, daemon=True)
        self._thread.start()
        return self

    def stop(self) -> None:
        self._httpd.shutdown()
        self._httpd.server_close()
        if self._thread is not None:
            self._thread.join(timeout=5)

    def __enter__(self) -> "FakeReleaseServer":
        return self.start()

    def __exit__(self, *_exc) -> None:
        self.stop()


def unused_loopback_url() -> str:
    """A loopback URL on a port that nothing listens on (connection refused)."""
    probe = ThreadingHTTPServer(("127.0.0.1", 0), BaseHTTPRequestHandler)
    port = probe.server_address[1]
    probe.server_close()
    return f"http://127.0.0.1:{port}"


def _main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    stage = commands.add_parser("stage", help="write a dummy release asset and checksum")
    stage.add_argument("--root", type=Path, required=True)
    stage.add_argument("--version", required=True)
    stage.add_argument("--asset", default=None)
    serve = commands.add_parser("serve", help="serve a release directory until killed")
    serve.add_argument("--root", type=Path, required=True)
    serve.add_argument("--port-file", type=Path, default=None)
    args = parser.parse_args(argv)

    if args.command == "stage":
        path = stage_release(args.root, args.version.lstrip("v"), args.asset)
        print(path)
        return 0

    server = FakeReleaseServer(args.root)
    if args.port_file is not None:
        args.port_file.write_text(str(server.port), encoding="ascii")
    print(server.base_url, flush=True)
    try:
        server._httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server._httpd.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(_main())
