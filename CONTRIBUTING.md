# Contributing to Neuron

Thanks for helping. This repository ships one product: the `neuron` CLI built from the
Rust workspace in `rust/` with its full-screen UI (`neuron-tui`, built from
`ui/neuron-tui` with Bun), plus the npm and Python launchers and install scripts that
deliver them.

## Repository layout

| Path | What it is |
| --- | --- |
| `rust/` | Cargo workspace; `crates/rusty-claude-cli` builds the `neuron` binary |
| `rust/crates/runtime/bundled-skills/` | Third-party skills embedded in the binary, with their licenses |
| `npm/neuroncli/` | npm package `@zero-x.live/neuron` (launcher only) |
| `neuron_cli/`, `pyproject.toml` | PyPI package `neuroncli` (launcher only) |
| `install.sh`, `install.ps1` | Release installers |
| `tests/` | Python and Node tests for the packaging surfaces, plus the Python porting workspace tests |
| `src/` | Python porting workspace inherited from Claw Code, used for parity tooling |
| `ui/neuron-tui/` | Full-screen terminal UI (Bun, OpenTUI, React), started by `neuron` on a terminal; see its README and PORTING.md |
| `docs/` | Design and product documents; `docs/GATEWAY_CONTRACT.md` is the CLI and gateway contract |

### The `opencode` entry

`opencode` is an orphan gitlink: the index records it as a submodule (commit
`73ee493`), but there is no `.gitmodules` entry and nothing in this branch builds from
it. It is left over from the Go `opencode` fork that lives on the `THE-BUGGY-BREACH`
branch. Because of it, `git submodule update` and `git clone --recurse-submodules`
fail with "no submodule mapping found in .gitmodules for path 'opencode'". Clone
without `--recurse-submodules`, and make tooling that walks submodules skip that path.
Removing it is a separate decision (see [docs/ISSUES_TRIAGE.md](docs/ISSUES_TRIAGE.md));
please do not include its removal in unrelated changes.

## Checks

Run what applies to your change before opening a pull request.

**Rust** (from `rust/`):

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings     # pedantic lints are errors
cargo test --workspace
```

Some tests change process-wide environment variables; if you see flaky failures, run
`cargo test --workspace -- --test-threads=1` as CI does on every platform.

**Packaging** (from the repository root; Python 3.9 or later and Node.js 22.14 or later):

```sh
python -m unittest discover -s tests -v            # launchers, installers, release workflow, metadata, porting workspace
cd npm/neuroncli && npm test && cd ../..           # npm launcher (node --test)
node --test tests/third_party_notices.test.mjs     # notices generator
NEURON_TEST_NPM_PACK=1 python -m unittest tests.test_npm_package -v   # npm pack + install; needs registry access
```

**Full-screen UI** (from `ui/neuron-tui`; Bun 1.3.14; see its README for `dev:mock` and
`check:console`):

```sh
bun install --frozen-lockfile
bun run typecheck
bun run test
```

The packaging tests run the real launchers and installers against a local fake release
server (`tests/fake_release.py`) serving a small dummy `neuron`. On Windows the dummy
is compiled with the .NET Framework `csc.exe` that ships with Windows; set
`NEURON_TEST_BASH` to Git for Windows' `usr\bin\bash.exe` to also run the `install.sh`
and release workflow step tests there. The porting workspace tests write their session
files to a temporary directory (`PORT_SESSIONS_DIR`), not to `.port_sessions/`.

To try an installer by hand against the fake server:

```sh
python tests/fake_release.py stage --root /tmp/releases --version 6.3.0
python tests/fake_release.py serve --root /tmp/releases --port-file /tmp/port &
NEURON_DOWNLOAD_BASE="http://127.0.0.1:$(cat /tmp/port)" NEURON_VERSION=6.3.0 \
  NEURON_INSTALL_DIR=/tmp/neuron-bin sh install.sh
```

**Third-party notices** (after changing Rust dependencies or the UI's npm dependencies):

```sh
cargo fetch --locked --manifest-path rust/Cargo.toml
(cd ui/neuron-tui && bun install --frozen-lockfile --production)
node scripts/generate-third-party-notices.mjs --output THIRD_PARTY_NOTICES.md
```

Pass `--no-ui` to leave the UI out when you have no Bun.

The generated file is a release asset; do not commit it.

## Pull requests

- Keep changes small and focused, with tests for every behavior change.
- Use [Conventional Commits](https://www.conventionalcommits.org/) subjects, for example
  `fix(install): ...` or `feat(cli): ...`.
- Add a line under the `unreleased` section of [CHANGELOG.md](CHANGELOG.md) for
  anything a user would notice.
- Update [USAGE.md](USAGE.md) or the README when commands, flags or install steps
  change.
- Do not bump versions in a feature pull request; releases do that (see
  [PUBLISHING.md](PUBLISHING.md)).
- Never commit credentials, session files or `.env` files. Report security problems
  privately as described in [SECURITY.md](SECURITY.md).

## Bundled skills

The skills under `rust/crates/runtime/bundled-skills/` come from other MIT-licensed
projects pinned in `SOURCES.json`. Refresh them with
`python scripts/vendor-default-skills.py`, keep each project's license file in
`licenses/`, and update [NOTICE](NOTICE) when you add or remove one.

## License

Neuron is MIT licensed (see [LICENSE](LICENSE) and [NOTICE](NOTICE)), except the
full-screen UI in `ui/neuron-tui`, which is derived from Freebuff and stays under the
Apache License 2.0 (see its `LICENSE` and `NOTICE`). By contributing, you agree that your
contribution is licensed under the terms of the part you change. A file in
`ui/neuron-tui` that differs from its Freebuff original keeps the "Derived from
Codebuff/Freebuff (Apache-2.0); modified for Neuron" header.
