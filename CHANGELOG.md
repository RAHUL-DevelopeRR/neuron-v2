# Changelog

Notable changes to Neuron. Versions follow [semantic versioning](https://semver.org/).
The release workflow copies a version's section into its GitHub release notes.

## 6.3.0 (unreleased)

### Installation and packaging

- New `install.sh` for Linux (x64, arm64) and macOS (Apple silicon, Intel): downloads
  the release binary and its `.sha256`, verifies it with `sha256sum` or `shasum`,
  checks that it runs, and installs it to `~/.local/bin` or `NEURON_INSTALL_DIR`. It
  warns when the directory is not on `PATH` or another `neuron` shadows it, and it
  replaces the old script that built from source and printed Claw Code text.
- New `install.ps1` for Windows x64, for Windows PowerShell 5.1 and PowerShell 7.
  Installs to `%LOCALAPPDATA%\Programs\Neuron\bin`; changes the user `PATH` only with
  `-AddToPath`. Under `irm | iex` it leaves the caller's session settings alone.
- Both installers accept a version (`NEURON_VERSION`, default: latest release) and a
  mirror (`NEURON_DOWNLOAD_BASE`), reject non-HTTPS mirrors (plain HTTP only to an
  exact loopback host, never with user info in the URL), and tell an unpublished
  version apart from a network failure. `GITHUB_TOKEN` is sent only to
  `api.github.com`, and `install.sh` keeps it off the command line.
- `install.sh` checks that the binary runs from the install directory, so a `/tmp`
  mounted `noexec` does not break it; `install.ps1` puts the previous `neuron.exe`
  back if the new one cannot be moved into place. Both print the installed version.
- The Python launcher (`neuroncli`) verifies the binary once when it downloads it,
  stores a `.sha256` next to it and runs it from the cache afterwards with no network
  access. Download problems print an actionable message instead of a traceback.
  Ctrl+C reaches Neuron instead of killing it, the exit code is passed through, and on
  Linux and macOS the launcher replaces itself with the binary. Prerelease versions
  download from their own tag (`v6.4.0-rc.1`).
- The npm launcher (`@zero-x.live/neuron`) downloads and verifies the binary on first
  run when install scripts were skipped (`--ignore-scripts`), falls back to the user
  cache when the package directory is read-only, forwards SIGTERM and SIGHUP (and
  SIGINT that did not come from the terminal) to Neuron, and exits with Neuron's code.
  A download fails only when no data arrives for 60 seconds, so slow links work.
- The PyPI wheel is a pure-Python launcher; it never contains a native binary.

### Release pipeline

- Release binaries for linux-x64, linux-arm64, macos-arm64, macos-x64 and windows-x64,
  each with a `.sha256` file, plus `SHA256SUMS`, `THIRD_PARTY_NOTICES.md` and both
  install scripts, with GitHub build provenance attestations. The Linux binaries are
  built on Ubuntu 22.04 and need glibc 2.35 or later.
- Before anything is published to npm or PyPI, the workflow HEAD-checks every public
  asset URL, verifies the checksums and installs from the release on Linux, macOS and
  Windows. The release becomes "Latest" only after those checks pass. npm and PyPI
  publishing stay off unless explicitly enabled, and the jobs that hold the publishing
  token install nothing and upload only the files that were tested.
- The assets of a published release are never rebuilt or replaced; a later run on the
  same tag only verifies them and publishes the packages.
- New Packaging CI runs the launcher, installer and npm tarball tests against a local
  fake release on Linux, macOS and Windows for every pull request that touches them.

### Licensing

- Added `LICENSE` (MIT), keeping the Claw Code copyright notice alongside Neuron's,
  and `NOTICE` describing where the code comes from and the licenses of the bundled
  skills. Both ship in the npm package and the Python wheel.
- `scripts/generate-third-party-notices.mjs` builds `THIRD_PARTY_NOTICES.md` (crate,
  version, license, repository and license texts) from `cargo metadata`.

### Documentation

- Rewrote the README, usage guide and release runbook for the Neuron product, and added
  a security policy, contributing guide, code provenance summary and an issue triage
  note. The security policy states plainly that the stored session file is
  obfuscated, not protected against copying.

## 6.2.5 and earlier

Versions up to 6.2.5 were built outside a reproducible release pipeline. The npm 6.2.0
and PyPI 6.2.4 (and earlier) packages do not match this repository's release binaries
and are deprecated; upgrade to 6.3.0 or later.
