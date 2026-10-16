"""Consistency checks across the packaging surfaces (manifests, launchers, workflows, docs)."""

from __future__ import annotations

import json
import re
import unittest
from pathlib import Path
from typing import Dict, Iterator, Set, Tuple

REPO_ROOT = Path(__file__).resolve().parents[1]
RELEASE_ASSETS = [
    "neuron-linux-x64",
    "neuron-linux-arm64",
    "neuron-macos-arm64",
    "neuron-macos-x64",
    "neuron-windows-x64.exe",
]
DOCS = [
    "README.md",
    "USAGE.md",
    "PUBLISHING.md",
    "CHANGELOG.md",
    "SECURITY.md",
    "CONTRIBUTING.md",
    "docs/PROVENANCE.md",
    "docs/ISSUES_TRIAGE.md",
    "npm/neuroncli/README.md",
]
UPSTREAM_COPYRIGHT = "Copyright (c) 2026 UltraWorkers and Claw Code contributors"
NEURON_COPYRIGHT = "Copyright (c) 2026 RAHUL S / Zero-X"
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
OWN_BLOB = "https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/HEAD/"


def read(relative: str) -> str:
    return (REPO_ROOT / relative).read_text(encoding="utf-8")


def toml_value(text: str, table: str, key: str) -> str:
    """A string value from a TOML table (enough for these manifests; tomllib needs 3.11)."""
    section = re.search(rf"^\[{re.escape(table)}\]\s*$(.*?)(?=^\[|\Z)", text, re.M | re.S)
    if section is None:
        raise AssertionError(f"[{table}] not found")
    match = re.search(rf'^{re.escape(key)}\s*=\s*"([^"]*)"', section.group(1), re.M)
    if match is None:
        raise AssertionError(f"{table}.{key} not found")
    return match.group(1)


def manifest_versions() -> Dict[str, str]:
    lock = json.loads(read("npm/neuroncli/package-lock.json"))
    launcher = re.search(r'^PACKAGE_VERSION = "([^"]+)"', read("neuron_cli/__init__.py"), re.M)
    assert launcher is not None
    return {
        "rust/Cargo.toml": toml_value(read("rust/Cargo.toml"), "workspace.package", "version"),
        "pyproject.toml": toml_value(read("pyproject.toml"), "project", "version"),
        "neuron_cli/__init__.py": launcher.group(1),
        "npm/neuroncli/package.json": json.loads(read("npm/neuroncli/package.json"))["version"],
        "npm/neuroncli/package-lock.json": lock["version"],
        "npm/neuroncli/package-lock.json packages['']": lock["packages"][""]["version"],
    }


def markdown_links(relative: str) -> Iterator[Tuple[int, str]]:
    text = read(relative)
    for match in LINK.finditer(text):
        yield text.count("\n", 0, match.start()) + 1, match.group(1)


def heading_anchors(path: Path) -> Set[str]:
    """GitHub-style anchors for the headings of a Markdown file."""
    anchors = set()
    in_fence = False
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("```"):
            in_fence = not in_fence
        heading = None if in_fence else re.match(r"^#{1,6}\s+(.*?)\s*#*\s*$", line)
        if heading:
            slug = re.sub(r"[^\w\- ]", "", heading.group(1).lower()).replace(" ", "-")
            anchors.add(slug)
    return anchors


class PackagingMetadataTests(unittest.TestCase):
    def test_every_manifest_carries_the_same_version(self) -> None:
        versions = manifest_versions()
        self.assertEqual(len(set(versions.values())), 1, versions)

    def test_license_keeps_the_upstream_notice_and_adds_neuron(self) -> None:
        license_text = read("LICENSE")
        self.assertTrue(license_text.startswith("MIT License\n"))
        self.assertIn(UPSTREAM_COPYRIGHT, license_text)
        self.assertIn(NEURON_COPYRIGHT, license_text)
        self.assertIn("The above copyright notice and this permission notice shall be included", license_text)
        notice = read("NOTICE")
        self.assertIn(UPSTREAM_COPYRIGHT, notice)
        self.assertIn("docs/PROVENANCE.md", notice)
        self.assertTrue((REPO_ROOT / "docs" / "PROVENANCE.md").is_file())

    def test_notice_lists_every_bundled_skill_and_its_license_file(self) -> None:
        skills = REPO_ROOT / "rust" / "crates" / "runtime" / "bundled-skills"
        sources = json.loads((skills / "SOURCES.json").read_text(encoding="utf-8"))
        notice = read("NOTICE")
        for name, source in sources.items():
            with self.subTest(skill=name):
                license_file = skills / "licenses" / f"{name}.txt"
                self.assertTrue(license_file.is_file())
                self.assertIn(source["repository"], notice)
                self.assertIn(f"licenses/{name}.txt", notice)
                copyright_line = next(
                    line for line in license_file.read_text(encoding="utf-8").splitlines() if line.startswith("Copyright")
                )
                self.assertIn(copyright_line, notice)

    def test_packages_ship_the_license_files(self) -> None:
        pyproject = read("pyproject.toml")
        self.assertRegex(pyproject, r'(?m)^license = "MIT"$')
        self.assertRegex(pyproject, r'(?m)^license-files = \["LICENSE", "NOTICE"\]$')
        # The wheel is a pure-Python launcher; a native binary must never be packed into it.
        self.assertNotIn("package-data", pyproject)
        package = json.loads(read("npm/neuroncli/package.json"))
        self.assertEqual(package["license"], "MIT")
        for name in ("LICENSE", "NOTICE"):
            self.assertIn(name, package["files"])
            self.assertEqual(read(f"npm/neuroncli/{name}"), read(name), f"npm/neuroncli/{name} drifted")

    def test_release_assets_match_every_installer(self) -> None:
        release = read(".github/workflows/release.yml")
        launchers = {
            "release.yml": release,
            "install.sh": read("install.sh"),
            "install.ps1": read("install.ps1"),
            "neuron_cli/__init__.py": read("neuron_cli/__init__.py"),
            "npm/neuroncli/lib/binary.js": read("npm/neuroncli/lib/binary.js"),
        }
        for asset in RELEASE_ASSETS:
            self.assertIn(asset, release)
        for name in ("neuron_cli/__init__.py", "npm/neuroncli/lib/binary.js"):
            for asset in RELEASE_ASSETS:
                self.assertIn(asset, launchers[name], f"{name} does not map {asset}")
        self.assertIn("neuron-windows-x64.exe", launchers["install.ps1"])
        for part in ("linux", "macos", "x64", "arm64"):
            self.assertIn(part, launchers["install.sh"])
        for runner in ("ubuntu-24.04-arm", "actions/attest-build-provenance", "SHA256SUMS", "THIRD_PARTY_NOTICES.md"):
            self.assertIn(runner, release)

    def test_publishing_stays_gated_behind_explicit_switches(self) -> None:
        release = read(".github/workflows/release.yml")
        for job, switch, flag in (
            ("publish-npm", "inputs.publish_npm", "vars.ENABLE_NPM_PUBLISH == 'true'"),
            ("publish-pypi", "inputs.publish_pypi", "vars.ENABLE_PYPI_PUBLISH == 'true'"),
        ):
            with self.subTest(job=job):
                block = release.split(f"\n  {job}:", 1)[1].split("\n  publish-", 1)[0]
                self.assertIn("needs: [metadata, readiness, install-check]", block)
                self.assertIn(switch, block)
                self.assertIn(flag, block)

    def test_changelog_leads_with_the_next_or_current_version(self) -> None:
        # The release notes come from the section named after the tag (release.yml).
        top = re.search(r"(?m)^## (.+)$", read("CHANGELOG.md"))
        assert top is not None
        heading = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)? \((unreleased|\d{4}-\d{2}-\d{2})\)", top.group(1))
        self.assertIsNotNone(heading, f"first changelog section must be 'X.Y.Z (unreleased)' or 'X.Y.Z (YYYY-MM-DD)': {top.group(1)}")
        assert heading is not None
        section = tuple(int(part) for part in heading.groups()[:3])
        manifest = tuple(int(part) for part in re.findall(r"\d+", manifest_versions()["rust/Cargo.toml"])[:3])
        if heading.group(4) == "unreleased":
            self.assertGreaterEqual(section, manifest)
        else:
            self.assertEqual(section, manifest, "a dated changelog section must match the manifest version")

    def test_documentation_links_resolve(self) -> None:
        for document in DOCS:
            base = (REPO_ROOT / document).parent
            for line, target in markdown_links(document):
                with self.subTest(document=document, line=line, target=target):
                    if target.startswith(OWN_BLOB):
                        path = REPO_ROOT / target[len(OWN_BLOB) :].split("#", 1)[0]
                    elif re.match(r"^[a-z]+:", target):
                        continue
                    elif target.startswith("#"):
                        path = REPO_ROOT / document
                    else:
                        path = base / target.split("#", 1)[0]
                    self.assertTrue(path.exists(), f"{document}:{line} links to a missing file: {target}")
                    fragment = target.split("#", 1)[1] if "#" in target else None
                    if fragment is not None and path.suffix == ".md":
                        self.assertIn(fragment, heading_anchors(path), f"{document}:{line} links to a missing heading")


if __name__ == "__main__":
    unittest.main()
