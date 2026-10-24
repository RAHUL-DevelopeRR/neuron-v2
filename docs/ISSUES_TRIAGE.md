# Issue triage: #1 and #2

Status as of 2026-10-16. Both issues were opened on 2026-05-22 and are still open.
Both describe fixes in the Go `opencode` fork, not in the Rust CLI this branch ships.

## Where the code they describe lives

This repository has two unrelated programs called Neuron:

| | Rust CLI | Go fork |
| --- | --- | --- |
| Branch | `ux-audit-fixes` (default) and the 7.0.0-alpha.1 work | `THE-BUGGY-BREACH` |
| Code | `rust/`, binary `neuron` from `rusty-claude-cli` | `go/`, module `github.com/opencode-ai/opencode` |
| Packaging | npm `@zero-x.live/neuron`, PyPI `neuroncli`, `install.sh`, `install.ps1`, GitHub Releases | None of its own; reached through the old Python shim |
| Gateway contract | Implements [GATEWAY_CONTRACT.md](GATEWAY_CONTRACT.md) | Predates it |

Every file the two issues name (`internal/config/config.go`, `getProviderAPIKey()`,
`models.ProviderNeuron`, `internal/tui/components/chat/chat.go`, lipgloss) exists only
under `go/` on `THE-BUGGY-BREACH`.

## #1: Gateway returned 42X errors because of binary resolution conflicts

**Reported.** Running `neuron` gave HTTP 422/429 gateway errors. The pip shim's
`_find_binary()` could resolve a stock `opencode.exe` installed by npm instead of the
custom build, and that binary used default providers. The proposed fixes skip API key
validation for the Neuron provider in the Go config, add a placeholder key for it, and
deploy the custom `Neuron.exe` to a fixed path.

**Analysis.**

- The resolution bug was real. The shim on `THE-BUGGY-BREACH` searches every directory
  on `PATH` (`os.get_exec_path()`) for `neuron.exe` (`neuron` elsewhere), so whatever
  carries that name and comes first wins: an `opencode` build installed as Neuron, an
  older package, or the pip entry-point shim itself.
- The Rust line no longer does this. Already at the 7.0.0-alpha.1 branch point the
  Python launcher ran only `NEURON_BINARY_PATH`, a binary next to the launcher, or the
  release binary for its own version. 7.0.0-alpha.1 adds a verified cache, actionable
  network errors and correct signal handling, and its tests check that other `neuron`
  executables on `PATH` are ignored (`tests/test_packaged_launcher.py`). The npm
  launcher resolves the same way, and `install.sh` and `install.ps1` warn when another
  `neuron` shadows the one they installed.
- The 42X responses themselves are the gateway rejecting a client that does not speak
  the contract. With the Rust CLI, 429 is either `insufficient_quota` (stop, show the
  reset time) or `rate_limited` (retry after `Retry-After`), both rendered with what to
  do next ([USAGE.md](../USAGE.md#plans-usage-and-limits)).
- The Go changes (skipping key validation, a hard-coded placeholder key, a fixed
  install path under one user's profile) do not apply to the Rust CLI.

**Recommendation.** Close as fixed for the Rust product line in 7.0.0-alpha.1 (launcher
resolution) and as not applicable for the Go-specific changes. Ask anyone still seeing
422/429 to open a new issue with `neuron --version`, `neuron doctor`, the install
method and the error's `code` and `request_id`.

## #2: Broken TUI banner alignment and centering

**Reported.** The block-letter "NEURON" banner rendered with lipgloss is misaligned
because block and box-drawing characters are East Asian "Ambiguous" width. The
proposed fix replaces lipgloss with raw ANSI spacing and a fixed six-cell letter grid
in `internal/tui/components/chat/chat.go`.

**Analysis.**

- lipgloss and that file belong to the Go fork; the Rust CLI has no lipgloss
  dependency.
- The Rust banner (`startup_banner` in `rust/crates/rusty-claude-cli/src/main.rs`) uses
  ASCII borders and the colored word "Neuron"; no Rust source uses block glyphs, so the
  ambiguous-width problem does not occur there.
- While checking, a different problem turned up in the Rust banner: it shortens long
  working directories with a byte slice (`&s[s.len() - 32..]`), which panics when the
  cut lands inside a multi-byte character, for example in a path containing non-ASCII
  letters. It also labels the provider "OpenRouter" when the session uses the Zero-X
  gateway. These should be fixed and tested in the Rust CLI, as their own issue.

**Recommendation.** Close as not applicable to the Rust product line, and open a new
issue for the banner's path truncation and provider label.

## Recommendation: one product line

Ship only the Rust CLI as Neuron.

- It is the code that the packages, installers, release pipeline, license notices and
  the Zero-X gateway contract are built for. The Go fork has none of these, and its
  top-level `LICENSE` drops the Claw Code notice that the Rust line must keep (see
  [PROVENANCE.md](PROVENANCE.md)).
- Two programs with the same name and package names caused #1 in the first place.
  Keeping one removes that class of bug.

Steps, each its own decision and change:

1. Close #1 and #2 with the analysis above, linking this document.
2. Freeze `THE-BUGGY-BREACH`: mark it unmaintained in its README, or archive it as a
   tag and delete the branch. Do not publish packages from it.
3. Remove the orphan `opencode` gitlink from the default branch in a dedicated commit
   (it breaks `git submodule` commands; see
   [CONTRIBUTING.md](../CONTRIBUTING.md#the-opencode-entry)).
4. File Rust issues for anything from the fork worth keeping, starting with the banner
   problems above.
