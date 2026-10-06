# npm and PyPI releases

Create a version tag matching rust/Cargo.toml, pyproject.toml, and npm/neuroncli/package.json (currently v6.2.5). The release workflow builds Windows x64, Linux x64, and macOS arm64 binaries, attaches each binary and SHA-256 file to the GitHub release, then publishes `@zero-x.live/neuron` to npm and `neuroncli` to PyPI. Package launchers download the matching release binary and verify its checksum.

Before the first publish:

1. PyPI publication is paused while account recovery is pending. When restored, configure the existing neuroncli project to trust GitHub Actions for repository RAHUL-DevelopeRR/neuron-v2, workflow .github/workflows/release.yml, and environment pypi, then set the repository variable `ENABLE_PYPI_PUBLISH=true`. Until then, release tags skip the PyPI job.
2. Configure the existing `@zero-x.live/neuron` npm package to trust the same GitHub repository and release workflow. Alternatively, publish from an authenticated maintainer CLI after the matching GitHub release assets exist. A granular `NPM_TOKEN` secret is only needed when using token-based CI publication.
3. Merge the release workflow and package files into the GitHub repository, then push the matching version tag to trigger release and publication.

Registry setup requires maintainer account access. Before pushing the version tag, verify both registry publishers; a CLI login does not authenticate GitHub Actions. To validate the native artifacts first, dispatch the release workflow on the candidate branch: manual dispatch builds all three platforms without publishing. Test an installation from the built wheel/npm tarball and verify `neuron --version`, authentication, and a gateway completion before marking the release ready.
