# Release runbook

How a Neuron version goes from a merged change to GitHub Releases, npm and PyPI. The
release workflow is [`.github/workflows/release.yml`](.github/workflows/release.yml);
everything a user installs (binaries, checksums, install scripts, license notices)
comes from the GitHub release it creates, and the npm and PyPI packages are launchers
that download those binaries.

Order of operations: **merge, tag, verify the assets, publish to npm and PyPI by hand
(or with an explicit switch), deprecate old builds.** Registry publishing never
happens before the public release assets have been checked.

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
| `metadata` | Checks that the tag and every manifest carry the same version |
| `build` | Builds and smoke-tests `neuron` for linux-x64, linux-arm64 (`ubuntu-24.04-arm`), macos-arm64, macos-x64 and windows-x64, writes `<asset>.sha256`, and attests build provenance |
| `notices` | Generates `THIRD_PARTY_NOTICES.md` from `cargo metadata` |
| `release` | Writes `SHA256SUMS`, creates a draft release with all assets, checks the uploaded list, then publishes the release |
| `readiness` | HEAD-checks every public asset URL and verifies the downloaded files against `SHA256SUMS` |
| `install-check` | Installs from the public release with `install.sh` (Linux x64 and arm64, macOS) and with `irm \| iex` (Windows PowerShell 5.1), then runs `neuron --version` |
| `publish-npm`, `publish-pypi` | Run only after `readiness` and `install-check`, and only when enabled (see below) |

A manual run (Actions, Release, Run workflow) on a branch only builds the binaries; on
a tag it re-runs the release and can publish.

## 4. Verify the assets

The release must have these 14 assets:

- `neuron-linux-x64`, `neuron-linux-arm64`, `neuron-macos-arm64`, `neuron-macos-x64`,
  `neuron-windows-x64.exe`
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

## 5. Publish to npm

Either dispatch the workflow on the tag (Actions, Release, Run workflow, "Use workflow
from" `v6.3.0`, tick `publish_npm`), or publish from a maintainer machine:

```sh
git switch --detach v6.3.0
cd npm/neuroncli
npm pack
npm install --global ./zero-x.live-neuron-6.3.0.tgz   # downloads the release binary
neuron --version
npm publish --access public                            # asks for your 2FA code
```

The dispatched job also installs the packed tarball against the live release before it
publishes, and publishes prereleases (`6.4.0-rc.1`) under the `next` tag.

## 6. Publish to PyPI (paused)

When PyPI access is back and the trusted publisher is configured, dispatch the workflow
on the tag with `publish_pypi` ticked. The job builds the sdist and wheel, checks that
the wheel contains only the launcher and its license files, installs it against the
live release, and uploads. By hand:

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
- Mark it as a prerelease (`gh release edit vX.Y.Z --prerelease`) so `install.sh` and
  `install.ps1`, which follow `releases/latest`, go back to the previous release.
- `npm deprecate` the bad version and yank it on PyPI, then ship a fixed patch release.
