# Release runbook

How a Neuron version goes from a merged change to GitHub Releases, npm and PyPI. The
release workflow is [`.github/workflows/release.yml`](.github/workflows/release.yml);
everything a user installs (binaries, checksums, install scripts, license notices)
comes from the GitHub release it creates, and the npm and PyPI packages are launchers
that download those binaries.

Order of operations: **merge, tag, verify the assets, publish to npm and PyPI by hand
(or with an explicit switch), deprecate old builds.** Registry publishing never
happens before the public release assets have been checked, and the assets of a
published release are never rebuilt or replaced.

## One-time setup

1. **GitHub environments.** Create environments named `npm` and `pypi` (Settings,
   Environments) with required reviewers, so a publish job waits for a maintainer.
2. **npm trusted publishing.** On npmjs.com, package `@zero-x.live/neuron`, Settings,
   Trusted Publisher: GitHub Actions, repository `RAHUL-DevelopeRR/neuron-v2`, workflow
   `release.yml`, environment `npm`. The publish job uses OIDC; no `NPM_TOKEN` secret
   is needed.
3. **PyPI trusted publishing.** Paused: PyPI publishing waits until access to the
   `neuroncli` project is recovered. Then add a trusted publisher on PyPI (Manage
   project, Publishing): owner `RAHUL-DevelopeRR`, repository `neuron-v2`, workflow
   `release.yml`, environment `pypi`.
4. **Automatic publishing (optional).** Repository variables `ENABLE_NPM_PUBLISH=true`
   and `ENABLE_PYPI_PUBLISH=true` let tag pushes publish after verification. Leave
   them unset to publish only by hand or by manual dispatch (recommended until the
   pipeline has shipped a few releases).
5. **Security reporting.** Enable private vulnerability reporting (Settings, Code
   security) so the link in [SECURITY.md](SECURITY.md) works.

## 1. Prepare the release

- Bump the version everywhere at once: `rust/Cargo.toml` (`[workspace.package]`),
  `rust/Cargo.lock`, `npm/neuroncli/package.json`, `npm/neuroncli/package-lock.json`,
  `pyproject.toml` and `PACKAGE_VERSION` in `neuron_cli/__init__.py`.
  `python -m unittest tests.test_packaging_metadata` fails if any of them disagree, and
  the release workflow refuses a tag that does not match.
- In [CHANGELOG.md](CHANGELOG.md), rename `## X.Y.Z (unreleased)` to
  `## X.Y.Z (YYYY-MM-DD)`. The release notes are taken from that section.
- Run the checks from [CONTRIBUTING.md](CONTRIBUTING.md#checks): Rust fmt, clippy and
  tests, the Python and Node packaging tests, and the notices generator.
- Check that the docs describe the binary you are about to ship. The README,
  [USAGE.md](USAGE.md) and [SECURITY.md](SECURITY.md) describe the 6.3.0 behavior
  that `rusty-claude-cli/tests/gateway_roundtrip.rs` exercises against a fake gateway
  and that was exercised live against a local gateway during integration. On the
  release candidate, confirm at least:
  - `neuron auth login` (browser and `--with-token`), `neuron auth status` and
    `neuron auth logout`, with the login kept in the OS credential store on macOS
    and Windows and in `~/.neuroncli/credentials.json` (mode 0600) on Linux;
  - `NEURON_TOKEN` with a session token and with a personal API key (`nzx_…`);
  - `neuron usage`, and `/usage`, `/upgrade`, `/login` and `/logout` in a session;
  - `/model` listing the gateway catalog, and `--provider` or the `provider` setting
    choosing a bring-your-own-key provider while signed in;
  - the gateway error handling in [USAGE.md](USAGE.md#plans-usage-and-limits):
    terminal codes stop at once, `capacity_exhausted` and `rate_limited` wait up to
    90 seconds per retry (180 seconds in total) with a countdown, other transient
    errors retry at most twice within 60 seconds;
  - credential scrubbing: the session token is not exported into the environment,
    and `NEURON_TOKEN` and other `ses_`/`nzx_` values are removed from the
    environment of shell commands, hooks and MCP servers;
  - `neuron system-prompt` in an empty directory stays under 10,000 characters (the
    bundled skills are listed, not pasted).

## 2. Merge

Open a pull request into the default branch (`ux-audit-fixes`). Rust CI and Packaging
CI must pass. Packaging CI runs the launchers, the installers and a packed npm tarball
against a local fake release on Linux, macOS and Windows.

## 3. Tag

From the merge commit on the default branch:

```sh
git switch ux-audit-fixes && git pull --ff-only
git tag -a v6.3.0 -m "Neuron 6.3.0"
git push origin v6.3.0
```

The tag push starts the release workflow:

| Job | What it does |
| --- | --- |
| `metadata` | Checks that the tag and every manifest carry the same version, and whether the tag's release is already published |
| `build` | Builds and smoke-tests `neuron` for linux-x64 and linux-arm64 (on Ubuntu 22.04: `ubuntu-22.04` and `ubuntu-22.04-arm`; fails if a binary needs a glibc newer than 2.35), macos-arm64, macos-x64 and windows-x64, and writes `<asset>.sha256` |
| `attest` | Records GitHub build provenance for the five binaries |
| `notices` | Generates `THIRD_PARTY_NOTICES.md` from `cargo metadata` |
| `release` | Writes `SHA256SUMS`, creates a draft release with all assets, checks the uploaded list, then publishes the release without marking it "Latest" |
| `readiness` | HEAD-checks every public asset URL and verifies the downloaded files against `SHA256SUMS` |
| `install-check` | Installs from the public release with `install.sh` (Linux x64 and arm64, macOS) and with `irm \| iex` (Windows PowerShell 5.1), then runs `neuron --version` |
| `promote` | Marks the release "Latest" once `install-check` passes, unless it is a prerelease, was withdrawn (marked as a prerelease) or a newer release is already latest. `install.sh`, `install.ps1` and the `releases/latest` links move to the new version only at this point |
| `package-npm`, `package-pypi` | Pack the npm tarball, or build the wheel and sdist, and install them against the live release. They have no OIDC token, and the npm install runs with `--ignore-scripts` followed by an `npm rebuild` of this package only |
| `publish-npm`, `publish-pypi` | Hold the trusted-publishing OIDC token. They check out nothing and install nothing; they check that the files are the ones the package job tested (SHA-256) and upload them. They run only when enabled (see below) |

Each binary is built from the tag once. When a run on a tag finds the tag's release
already published (a later manual run, or a re-run), it skips `build`, `attest`,
`notices` and `release`, leaves the public assets and checksums alone, and goes on to
`readiness`, `install-check`, `promote` and whichever publish jobs are switched on. A
draft left by a failed run is not public, so its assets are rebuilt and replaced. A
manual run (Actions, Release, Run workflow) on a branch only builds the binaries.

## 4. Verify the assets

The release must have these 24 assets:

- `neuron-linux-x64`, `neuron-linux-arm64`, `neuron-macos-arm64`, `neuron-macos-x64`,
  `neuron-windows-x64.exe`
- the full-screen UI for each target: `neuron-tui-linux-x64`, `neuron-tui-linux-arm64`,
  `neuron-tui-macos-arm64`, `neuron-tui-macos-x64`, `neuron-tui-windows-x64.exe`
  (built from `ui/neuron-tui` with Bun in the same build job)
- one `.sha256` file for each of them
- `SHA256SUMS`, `THIRD_PARTY_NOTICES.md`, `install.sh`, `install.ps1`

Check them yourself before publishing anywhere else:

```sh
gh release view v6.3.0 --repo RAHUL-DevelopeRR/neuron-v2
gh release download v6.3.0 --repo RAHUL-DevelopeRR/neuron-v2 --dir neuron-6.3.0
cd neuron-6.3.0 && sha256sum --check SHA256SUMS
gh attestation verify neuron-linux-x64 --repo RAHUL-DevelopeRR/neuron-v2
```

On a clean machine of each kind you can reach, install with the documented commands,
then run `neuron --version`, `neuron auth login`, a short prompt through the gateway
and `/usage`.

Once `promote` has passed, `gh release view --repo RAHUL-DevelopeRR/neuron-v2` (no tag)
shows the new version. If `promote` did not run or skipped the release, mark it latest
yourself when you are satisfied: `gh release edit v6.3.0 --latest`.

## 5. Publish to npm

Either dispatch the workflow on the tag (Actions, Release, Run workflow, "Use workflow
from" `v6.3.0`, tick `publish_npm`), or publish from a maintainer machine:

```sh
git switch --detach v6.3.0
cd npm/neuroncli
npm pack
# --ignore-scripts: dependency install scripts never run where your npm login is
npm install --global --ignore-scripts ./zero-x.live-neuron-6.3.0.tgz
npm rebuild --global @zero-x.live/neuron   # this package's postinstall: downloads the binary
neuron --version
npm publish ./zero-x.live-neuron-6.3.0.tgz --access public   # asks for your 2FA code
```

The dispatched run does not rebuild or re-upload anything on the published release:
`package-npm` packs and tests the tarball, then `publish-npm` publishes exactly that
file. Prereleases (`6.4.0-rc.1`) go to the `next` dist-tag.

## 6. Publish to PyPI (paused)

When PyPI access is back and the trusted publisher is configured, dispatch the workflow
on the tag with `publish_pypi` ticked. `package-pypi` builds the sdist and wheel,
checks that the wheel contains only the launcher and its license files and installs it
against the live release; `publish-pypi` then uploads those exact files. By hand:

```sh
python -m build && python -m twine check --strict dist/*
python -m zipfile -l dist/neuroncli-6.3.0-py3-none-any.whl   # neuron_cli/__init__.py + dist-info only
python -m twine upload dist/*
```

## 7. Deprecate old builds

The builds that predate this pipeline do not match the release binaries:

```sh
npm deprecate @zero-x.live/neuron@6.2.0 "Unsupported build; install @zero-x.live/neuron@latest (6.3.0 or later)"
npm dist-tag ls @zero-x.live/neuron    # latest must point at the new version
```

On PyPI, yank `neuroncli` 6.2.4 and older (project, Manage, the release, Options,
Yank) with the reason "Unsupported build; install 6.3.0 or later". Yanked versions stay
installable when pinned exactly, so existing pins keep working.

## 8. After the release

- Add a new `## X.Y.Z (unreleased)` section to the changelog.
- Announce the release with the install commands from the release notes.

## If a release is broken

- Do not delete a release, its assets or its tag once npm or PyPI packages for that
  version exist: the launchers download that exact version's assets.
- Do not replace its assets either; the workflow refuses to. A checksum that changes
  after publication looks like tampering to everyone who verified the old one.
- Mark it as a prerelease (`gh release edit vX.Y.Z --prerelease`) so `install.sh` and
  `install.ps1`, which follow `releases/latest`, go back to the previous release; if
  that one is not marked latest again, run `gh release edit vX.Y.W --latest` for it.
- `npm deprecate` the bad version and yank it on PyPI, then ship a fixed patch release.
