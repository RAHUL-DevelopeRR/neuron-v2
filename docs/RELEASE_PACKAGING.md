# NeuronCLI Cross-OS Packaging

NeuronCLI ships the Go TUI/runtime as the canonical native binary. Package
managers are thin installers/launchers around signed GitHub Release assets.

## Install Channels

- npm: `npm install -g @zero-x/neuron`
- PyPI/pipx: `pipx install neuroncli`
- Homebrew: `brew install zero-x-live/neuron/neuroncli`
- Linux deb/rpm: install the matching package from GitHub Releases
- Windows: install the `.msi` when available or unzip the Windows archive
- Raw archives: download the matching `.tar.gz` or `.zip` from GitHub Releases

## Supported Targets

- `windows-amd64`
- `darwin-amd64`
- `darwin-arm64`
- `linux-amd64`
- `linux-arm64`

## Release Assets

The release pipeline publishes:

- raw binaries: `neuron-<goos>-<goarch>[.exe]`
- archives: `neuroncli-<version>-<target>.tar.gz` or `.zip`
- Linux packages: `.deb` and `.rpm` when nFPM is available
- Windows package: `.msi` when WiX is available
- npm tarballs for the main launcher and each platform package
- PyPI source/wheel distributions
- `checksums.txt`
- generated Homebrew formula

## Local Dry Run

```powershell
.\scripts\package-all.ps1 -Clean
```

Skip expensive tests during packaging iteration:

```powershell
.\scripts\package-all.ps1 -Clean -SkipTests
```

Publish is opt-in:

```powershell
.\scripts\package-all.ps1 -Clean -Publish
```

## Version Bump

```powershell
node .\scripts\bump.mjs 6.3.0
git tag v6.3.0
git push origin v6.3.0
```

The tag triggers `.github/workflows/release.yml`.

## Runtime Diagnostics

Users can inspect their install with:

```shell
neuron doctor
neuron update
```

`doctor` reports OS, shell, terminal, binary path, install source, version,
auth status, gateway health, and the correct update command for the detected
package manager.
