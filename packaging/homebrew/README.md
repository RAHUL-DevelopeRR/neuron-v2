# Homebrew Packaging

Run `scripts/package-all.ps1` from the repository root. It builds platform
archives under `dist/packages` and writes `packaging/homebrew/neuroncli.rb`
with SHA256 values for the generated archives.

Upload the archives to a GitHub release named `v<version>`, then copy the
generated formula into the Homebrew tap.
