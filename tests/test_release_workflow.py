"""Checks for .github/workflows/release.yml.

Structural checks read the workflow text (the standard library has no YAML
parser). Behaviour checks extract the shell of individual steps and run it with
bash against a fake ``gh`` that records its arguments, so the decisions about an
existing release, asset replacement and the "Latest" mark are exercised for real.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from typing import Dict, List, Optional

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fake_release  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (REPO_ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
OIDC_JOBS = {"attest", "publish-npm", "publish-pypi"}

# Answers `gh release view` from FAKE_GH_* variables and logs every call.
FAKE_GH = r"""#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_GH_LOG"
case "$1 $2" in
    "release view")
        shift 2
        tag=""
        case "${1:-}" in
            --*) ;;
            *) tag="$1" ;;
        esac
        case "$*" in
            *"--json isDraft"*)
                case "${FAKE_GH_STATE:-none}" in
                    none) echo "release not found" >&2; exit 1 ;;
                    draft) echo true ;;
                    *) echo false ;;
                esac
                ;;
            *"--json isPrerelease"*) echo "${FAKE_GH_PRERELEASE:-false}" ;;
            *"--json tagName"*)
                [ -z "$tag" ] || exit 2
                [ -n "${FAKE_GH_LATEST:-}" ] || { echo "release not found" >&2; exit 1; }
                echo "$FAKE_GH_LATEST"
                ;;
            *) exit 3 ;;
        esac
        ;;
esac
exit 0
"""


def bash() -> Optional[str]:
    """bash for the step scripts: NEURON_TEST_BASH on Windows (never the WSL launcher)."""
    if os.name == "nt":
        configured = os.environ.get("NEURON_TEST_BASH")
        return configured if configured and Path(configured).is_file() else None
    return shutil.which("bash")


def shell_path(path: Path) -> str:
    if os.name != "nt":
        return str(path)
    drive, rest = str(path).split(":", 1)
    return "/" + drive.lower() + rest.replace("\\", "/")


def job_block(name: str) -> str:
    match = re.search(rf"(?ms)^  {re.escape(name)}:\n(.*?)(?=^  \S|\Z)", WORKFLOW)
    if match is None:
        raise AssertionError(f"job {name} not found in release.yml")
    return match.group(1)


def job_names() -> List[str]:
    jobs = WORKFLOW.split("\njobs:\n", 1)[1]
    return re.findall(r"(?m)^  ([a-z][a-z0-9-]*):$", jobs)


def job_condition(name: str) -> str:
    """The job-level `if:` expression, folded onto one line."""
    lines = job_block(name).splitlines()
    for index, line in enumerate(lines):
        if line.startswith("    if: "):
            value = line[len("    if: ") :]
            if value != ">-":
                return value
            body = []
            for following in lines[index + 1 :]:
                if not following.startswith("      "):
                    break
                body.append(following.strip())
            return " ".join(body)
    return ""


def step_script(job: str, step: str) -> str:
    """The `run: |` shell of a named step, dedented."""
    lines = job_block(job).splitlines()
    for index, line in enumerate(lines):
        if line.strip() != f"- name: {step}":
            continue
        step_indent = len(line) - len(line.lstrip())
        for position in range(index + 1, len(lines)):
            current = lines[position]
            indent = len(current) - len(current.lstrip())
            if current.strip().startswith("- ") and indent == step_indent:
                break
            if current.strip() == "run: |":
                body = []
                for following in lines[position + 1 :]:
                    if following.strip() and len(following) - len(following.lstrip()) <= indent:
                        break
                    body.append(following)
                return textwrap.dedent("\n".join(body)) + "\n"
        break
    raise AssertionError(f"step {step!r} of job {job} has no run script")


class ReleaseWorkflowStructureTests(unittest.TestCase):
    def test_only_jobs_without_third_party_code_hold_an_oidc_token(self) -> None:
        holders = {name for name in job_names() if "id-token: write" in job_block(name)}
        self.assertEqual(holders, OIDC_JOBS)
        self.assertNotIn("id-token", WORKFLOW.split("\njobs:\n", 1)[0], "no workflow-level OIDC permission")
        for name in OIDC_JOBS:
            block = job_block(name)
            with self.subTest(job=name):
                for forbidden in ("actions/checkout", "npm install", "npm ci", "npm rebuild", "pip install", "cargo "):
                    self.assertNotIn(forbidden, block)
        for name in ("publish-npm", "publish-pypi"):
            with self.subTest(job=name):
                self.assertIn("sha256sum", job_block(name), "the publish job must check it got the tested files")

    def test_package_jobs_test_without_running_dependency_install_scripts(self) -> None:
        npm_block = job_block("package-npm")
        install = next(line for line in npm_block.splitlines() if "npm install --global" in line)
        self.assertIn("--ignore-scripts", install)
        self.assertIn("npm rebuild --global", npm_block)
        self.assertIn("persist-credentials: false", npm_block)
        self.assertIn("npm publish \"dist/$TARBALL\"", job_block("publish-npm"))
        self.assertIn("--ignore-scripts", job_block("publish-npm"))
        self.assertIn("pypa/gh-action-pypi-publish", job_block("publish-pypi"))

    def test_publishing_stays_gated_behind_explicit_switches(self) -> None:
        for registry, switch, variable in (
            ("npm", "inputs.publish_npm", "vars.ENABLE_NPM_PUBLISH == 'true'"),
            ("pypi", "inputs.publish_pypi", "vars.ENABLE_PYPI_PUBLISH == 'true'"),
        ):
            for job, needs, upstream in (
                (f"package-{registry}", "needs: [metadata, install-check]", "needs.install-check.result == 'success'"),
                (f"publish-{registry}", f"needs: [metadata, package-{registry}]", f"needs.package-{registry}.result == 'success'"),
            ):
                with self.subTest(job=job):
                    condition = job_condition(job)
                    self.assertIn(needs, job_block(job))
                    self.assertIn(upstream, condition)
                    self.assertIn(switch, condition)
                    self.assertIn(variable, condition)
        self.assertIn("environment:\n      name: npm", job_block("publish-npm"))
        self.assertIn("environment:\n      name: pypi", job_block("publish-pypi"))

    def test_assets_are_built_only_while_the_release_is_unpublished(self) -> None:
        for job in ("build", "attest", "notices", "release"):
            with self.subTest(job=job):
                self.assertIn("needs.metadata.outputs.build == 'true'", job_condition(job))
        self.assertEqual(WORKFLOW.count("--clobber"), 1)
        self.assertIn("--clobber", job_block("release"))
        readiness = job_condition("readiness")
        self.assertIn("needs.release.result == 'skipped' && needs.metadata.outputs.build == 'false'", readiness)
        self.assertIn("!cancelled()", readiness)

    def test_the_release_becomes_latest_only_after_the_install_checks(self) -> None:
        self.assertIn('gh release edit "$TAG" --draft=false --latest=false', job_block("release"))
        self.assertIn("needs: [metadata, install-check]", job_block("promote"))
        self.assertIn("needs.install-check.result == 'success'", job_condition("promote"))
        self.assertIn("needs.metadata.outputs.prerelease == 'false'", job_condition("promote"))
        self.assertIn("needs: [metadata, readiness]", job_block("install-check"))

    def test_every_downstream_job_survives_skipped_build_jobs(self) -> None:
        # A job whose needs include a skipped job is skipped too unless its condition
        # uses a status function, so every job after release must say !cancelled().
        for job in ("readiness", "install-check", "promote", "package-npm", "publish-npm", "package-pypi", "publish-pypi"):
            with self.subTest(job=job):
                self.assertIn("!cancelled()", job_condition(job))


@unittest.skipUnless(bash(), "needs bash (set NEURON_TEST_BASH on Windows)")
class ReleaseWorkflowStepTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temporary = tempfile.TemporaryDirectory()
        self.root = Path(self._temporary.name)
        self.fakebin = self.root / "fakebin"
        fake_release.write_executable(self.fakebin / "gh", FAKE_GH.encode())
        self.log = self.root / "gh.log"
        self.log.touch()

    def tearDown(self) -> None:
        self._temporary.cleanup()

    def run_step(self, job: str, step: str, files: Optional[Dict[str, str]] = None, **env: str) -> subprocess.CompletedProcess:
        script = step_script(job, step)
        self.assertNotIn("${{", script, "the step must read its inputs from the environment")
        work = self.root / "work"
        work.mkdir(exist_ok=True)
        for name, content in (files or {}).items():
            (work / name).parent.mkdir(parents=True, exist_ok=True)
            (work / name).write_text(content, encoding="utf-8")
        (self.root / "step.sh").write_text(script, encoding="utf-8", newline="\n")
        output = self.root / "github_output"
        output.write_text("", encoding="utf-8")
        environment = {key: value for key, value in os.environ.items() if not key.startswith(("FAKE_GH_", "GITHUB_"))}
        environment.update(
            PATH=os.pathsep.join([str(self.fakebin), environment.get("PATH", "")]),
            FAKE_GH_LOG=shell_path(self.log),
            GITHUB_OUTPUT=shell_path(output),
            GITHUB_STEP_SUMMARY=shell_path(self.root / "summary"),
            GITHUB_REPOSITORY="RAHUL-DevelopeRR/neuron-v2",
            **env,
        )
        bash_path = bash()
        assert bash_path is not None
        return subprocess.run(
            [bash_path, "-e", shell_path(self.root / "step.sh")],
            cwd=work,
            env=environment,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )

    def gh_calls(self) -> List[str]:
        return self.log.read_text(encoding="utf-8").splitlines()

    def outputs(self) -> Dict[str, str]:
        lines = (self.root / "github_output").read_text(encoding="utf-8").splitlines()
        return dict(line.split("=", 1) for line in lines if "=" in line)

    def test_metadata_skips_the_build_only_for_a_published_release(self) -> None:
        cases = {
            ("true", "none"): ("none", "true"),
            ("true", "draft"): ("draft", "true"),
            ("true", "published"): ("published", "false"),
            ("false", "published"): ("none", "true"),  # a branch run never looks up a release
        }
        for (is_release, state), (expected_state, expected_build) in cases.items():
            with self.subTest(is_release=is_release, state=state):
                result = self.run_step(
                    "metadata",
                    "Look up an existing release for the tag",
                    IS_RELEASE=is_release,
                    TAG="v6.3.0",
                    FAKE_GH_STATE=state,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.outputs(), {"release_state": expected_state, "build": expected_build})

    def run_upload(self, state: str) -> subprocess.CompletedProcess:
        self.log.write_text("", encoding="utf-8")
        return self.run_step(
            "release",
            "Upload assets to a draft release",
            files={"dist/neuron-linux-x64": "binary", "notes.md": "notes"},
            TAG="v6.3.0",
            VERSION="6.3.0",
            PRERELEASE="false",
            FAKE_GH_STATE=state,
        )

    def test_a_published_release_is_never_uploaded_to(self) -> None:
        result = self.run_upload("published")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("never replaced", result.stdout + result.stderr)
        self.assertFalse([call for call in self.gh_calls() if " upload " in f" {call} " or "create" in call])

    def test_only_a_draft_release_has_its_assets_replaced(self) -> None:
        draft = self.run_upload("draft")
        self.assertEqual(draft.returncode, 0, draft.stderr)
        self.assertIn("release upload v6.3.0 dist/neuron-linux-x64 --clobber", self.gh_calls())

        fresh = self.run_upload("none")
        self.assertEqual(fresh.returncode, 0, fresh.stderr)
        create = [call for call in self.gh_calls() if call.startswith("release create v6.3.0")]
        self.assertEqual(len(create), 1, self.gh_calls())
        self.assertIn("--draft", create[0])
        self.assertNotIn("--clobber", " ".join(self.gh_calls()))

    def run_promote(self, latest: str = "", prerelease: str = "false", version: str = "6.3.0") -> List[str]:
        self.log.write_text("", encoding="utf-8")
        result = self.run_step(
            "promote",
            "Mark the release as latest unless it was withdrawn or a newer one is",
            TAG=f"v{version}",
            VERSION=version,
            FAKE_GH_LATEST=latest,
            FAKE_GH_PRERELEASE=prerelease,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return [call for call in self.gh_calls() if call.startswith("release edit")]

    def test_promote_marks_a_new_release_latest(self) -> None:
        edit = ["release edit v6.3.0 --repo RAHUL-DevelopeRR/neuron-v2 --latest"]
        self.assertEqual(self.run_promote(latest="v6.2.9"), edit)
        self.assertEqual(self.run_promote(latest="v6.3.0"), edit)
        self.assertEqual(self.run_promote(latest=""), edit)  # the first release of the repository
        self.assertEqual(
            self.run_promote(latest="v6.9.0", version="6.10.0"),
            ["release edit v6.10.0 --repo RAHUL-DevelopeRR/neuron-v2 --latest"],
        )

    def test_promote_leaves_a_newer_or_withdrawn_release_alone(self) -> None:
        self.assertEqual(self.run_promote(latest="v6.4.0"), [])  # publishing an old tag after the fact
        self.assertEqual(self.run_promote(latest="v6.2.9", prerelease="true"), [])


if __name__ == "__main__":
    unittest.main()
