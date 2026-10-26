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
3. **PyPI trusted publishing.** On PyPI, project `neuroncli`, Manage project,
   Publishing, add a trusted publisher: owner `RAHUL-DevelopeRR`, repository
   `neuron-v2`, workflow `release.yml`, environment `pypi`. The publish job uses OIDC;
   no PyPI token is needed.
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
  `python -m unittest discover -s tests -p test_packaging_metadata.py` fails if any of
  them disagree, and the release workflow refuses a tag that does not match.
- A version with a suffix, such as `7.0.0-alpha.2`, is a prerelease. Spell it the same
  way in every one of those files, `pyproject.toml` and `PACKAGE_VERSION` included:
  the workflow and the launchers compare the raw string, and only the built wheel and
  sdist file names use the PEP 440 form (`7.0.0a2`). The workflow publishes a
  prerelease on GitHub without ever marking it "Latest". On npm, a tag push publishes
  it under the `next` dist-tag; a manual run publishes it under the `npm_dist_tag` you
  choose (see [Publish to npm](#5-publish-to-npm)). PyPI has no dist-tags: pip installs
  a pre-release only with `--pre` or an exact pin.
- In [CHANGELOG.md](CHANGELOG.md), rename `## X.Y.Z (unreleased)` to
  `## X.Y.Z (YYYY-MM-DD)`. The release notes are taken from that section.
- Run the checks from [CONTRIBUTING.md](CONTRIBUTING.md#checks): Rust fmt, clippy and
  tests, the Python and Node packaging tests, and the notices generator.
- Check that the docs describe the binary you are about to ship. The README,
  [USAGE.md](USAGE.md) and [SECURITY.md](SECURITY.md) describe the 7.0.0-alpha.2 behavior
  that `rusty-claude-cli/tests/gateway_roundtrip.rs` exercises against a fake gateway
  and that was exercised live against a local gateway during integration. On the
  release candidate, confirm at least:
  - `neuron auth login` (device code and `--with-token`), `neuron auth status` and
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
    and `NEURON_TOKEN`, provider keys, `*_API_KEY`, `*_TOKEN` and `*_SECRET`
    variables and other `ses_`/`nzx_` values are removed from the environment of
    shell commands, hooks and MCP servers;
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
git tag -a v7.0.0-alpha.2 -m "Neuron 7.0.0-alpha.2"
git push origin v7.0.0-alpha.2
```

The tag push starts the release workflow:

| Job | What it does |
| --- | --- |
| `metadata` | Checks that the tag and every manifest carry the same version, and whether the tag's release is already published |
| `build` | Builds and smoke-tests `neuron` and `neuron-tui` for linux-x64 and linux-arm64 (on Ubuntu 22.04: `ubuntu-22.04` and `ubuntu-22.04-arm`; fails if a binary needs a glibc newer than 2.35), macos-arm64, macos-x64 and windows-x64, and writes `<asset>.sha256` |
| `attest` | Records GitHub build provenance for the ten binaries (`neuron` and `neuron-tui` for each target) |
| `notices` | Generates `THIRD_PARTY_NOTICES.md` from `cargo metadata` and the UI's installed npm packages |
| `release` | Writes `SHA256SUMS`, creates a draft release with all assets, checks the uploaded list, then publishes the release without marking it "Latest" |
| `readiness` | HEAD-checks every public asset URL and verifies the downloaded files against `SHA256SUMS` |
| `install-check` | Installs from the public release with `install.sh` (Linux x64 and arm64, macOS) and with `irm \| iex` (Windows PowerShell 5.1), pinned to the release's version, then runs `neuron --version` and `neuron-tui --version` |
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
gh release view v7.0.0-alpha.2 --repo RAHUL-DevelopeRR/neuron-v2
gh release download v7.0.0-alpha.2 --repo RAHUL-DevelopeRR/neuron-v2 --dir neuron-7.0.0-alpha.2
cd neuron-7.0.0-alpha.2 && sha256sum --check SHA256SUMS
gh attestation verify neuron-linux-x64 --repo RAHUL-DevelopeRR/neuron-v2
```

On a clean machine of each kind you can reach, install with the commands in the
release notes, which name the version (a prerelease has no `releases/latest` link),
then run `neuron --version`, `neuron auth login`, a short prompt through the gateway
and `/usage`.

Once `promote` has passed, `gh release view --repo RAHUL-DevelopeRR/neuron-v2` (no tag)
shows the new version. If `promote` did not run or skipped a stable release, mark it
latest yourself when you are satisfied: `gh release edit vX.Y.Z --latest`. Never do
this for a prerelease such as `v7.0.0-alpha.2`: `promote` skips prereleases on
purpose, and a prerelease stays out of `releases/latest`.

## 5. Publish to npm

Either dispatch the workflow on the tag (Actions, Release, Run workflow, "Use workflow
from" `v7.0.0-alpha.2`, tick `publish_npm` and choose the `npm_dist_tag`), or publish
from a maintainer machine.

The dist-tag decides what a plain `npm install --global @zero-x.live/neuron` gets:

- `next` (the default of a manual run) publishes the version without moving `latest`;
  users get it only with `@next`. Use it for a prerelease that should not be the
  default install yet.
- `latest` makes it the default install. 7.0.0-alpha.2 is published this way, so that
  the default install is the new Neuron instead of the unsupported 6.2.0. Choose
  `latest` for every stable release as well: a manual run never picks it for you.

A tag push with `ENABLE_NPM_PUBLISH` set keeps the automatic rule: a prerelease goes
to `next` and a stable release to `latest`. Either way the workflow passes `--tag`
explicitly, because npm 11 refuses to publish a prerelease without one.

By hand:

```sh
git switch --detach v7.0.0-alpha.2
cd npm/neuroncli
npm pack
# --ignore-scripts: dependency install scripts never run where your npm login is
npm install --global --ignore-scripts ./zero-x.live-neuron-7.0.0-alpha.2.tgz
npm rebuild --global @zero-x.live/neuron   # this package's postinstall: downloads the binary
neuron --version
# always name the dist-tag: npm 11 refuses a prerelease without --tag
npm publish ./zero-x.live-neuron-7.0.0-alpha.2.tgz --access public --tag latest   # asks for your 2FA code
npm dist-tag ls @zero-x.live/neuron   # latest: 7.0.0-alpha.2
```

Use `--tag next` instead for a prerelease that should not become the default install.

The dispatched run does not rebuild or re-upload anything on the published release:
`package-npm` packs and tests the tarball, then `publish-npm` publishes exactly that
file under the chosen dist-tag.

## 6. Publish to PyPI

Once the trusted publisher is configured (one-time setup, step 3), dispatch the
workflow on the tag with `publish_pypi` ticked. `package-pypi` builds the sdist and
wheel, checks that the wheel contains only the launcher and its license files and
installs it against the live release; `publish-pypi` then uploads those exact files.
By hand:

```sh
python -m build && python -m twine check --strict dist/*
python -m zipfile -l dist/neuroncli-7.0.0a2-py3-none-any.whl   # neuron_cli/__init__.py + dist-info only (PEP 440 spells 7.0.0-alpha.2 as 7.0.0a2)
python -m twine upload dist/*
```

PyPI has no dist-tags. A prerelease such as 7.0.0-alpha.2 (`7.0.0a2`) is installed with
`pip install --pre neuroncli` or an exact pin; plain `pip install neuroncli` keeps
choosing an older stable version until 7.0.0 is out, which is why the README says
`--pre`.

## 7. Deprecate old builds

The builds that predate this pipeline (npm 6.2.0, PyPI 6.2.4 and earlier) do not match
the release binaries. Deprecate them once the new version is the default install on
npm and is live on PyPI: from 7.0.0-alpha.2 on, npm's `latest` is the new Neuron, so a
deprecation message that points at `latest` now points at a supported build.

The maintainer (Rahul) runs these by hand, after the npm and PyPI publishes of
7.0.0-alpha.2 have succeeded; the workflow never deprecates or yanks anything:

```sh
npm dist-tag ls @zero-x.live/neuron    # latest must point at 7.0.0-alpha.2 (or newer) first
npm view @zero-x.live/neuron versions  # the old builds the range below covers
npm deprecate "@zero-x.live/neuron@<7.0.0-0" "Unsupported build; install @zero-x.live/neuron@latest (7.0.0-alpha.2 or later)"
```

`<7.0.0-0` covers 6.2.0 and every earlier version, and no 7.0.0 prerelease. PyPI has
no command-line yank (twine cannot do it): on pypi.org, project `neuroncli`, Manage,
open 6.2.4 and each older release, Options, Yank, with the reason "Unsupported build;
install 7.0.0-alpha.2 or later with pip install --pre neuroncli". Yanked versions stay
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
