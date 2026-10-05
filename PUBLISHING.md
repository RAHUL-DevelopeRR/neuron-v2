# npm and PyPI releases

Create a version tag matching rust/Cargo.toml, pyproject.toml, and npm/neuroncli/package.json (currently v6.2.5). The release workflow builds Windows x64, Linux x64, and macOS arm64 binaries, attaches each binary and SHA-256 file to the GitHub release, then publishes `@zero-x/neuron` to npm and `neuroncli` to PyPI. Package launchers download the matching release binary and verify its checksum.

Before the first publish:

1. Configure the existing neuroncli PyPI project to trust GitHub Actions for repository RAHUL-DevelopeRR/neuron-v2, workflow .github/workflows/release.yml, and environment pypi.
2. Add an npm granular publish token as the GitHub Actions secret NPM_TOKEN for the first npm release. After `@zero-x/neuron` exists, configure it to trust the same GitHub repository and release workflow, then remove NPM_TOKEN; later publishes use npm trusted publishing.
3. Merge the release workflow and package files into the GitHub repository, then push the matching version tag to trigger release and publication.

Registry setup requires account access. This checkout currently has no npm login, npm token, or PyPI publishing credentials, so it cannot publish. Do not push the tag until the PyPI publisher and NPM_TOKEN secret are configured.
