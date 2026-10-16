# Code provenance

Where the code and content in this repository come from, under which terms, and what
should be reviewed before Neuron is sold as a paid product. This is an engineering
summary, not legal advice.

## Summary

| Part | Origin | License | Notes |
| --- | --- | --- | --- |
| `rust/` workspace, including the `rusty-claude-cli` crate that builds `neuron` | [Claw Code](https://github.com/ultraworkers/claw-code), modified since | MIT, "Copyright (c) 2026 UltraWorkers and Claw Code contributors" | Zero-X login, the encrypted session vault, branding and gateway routing were added here. Crate and module names (`rusty-claude-cli`, `claw` settings paths) are inherited. |
| `src/`, `tests/test_porting_workspace.py` | Claw Code's Python porting workspace | MIT (as above) | See [reference data snapshots](#reference-data-snapshots). |
| `PARITY.md`, `PHILOSOPHY.md`, `ROADMAP.md` and other parity documents | Claw Code | MIT (as above) | Describe the upstream project and its `claw` commands. |
| `rust/crates/runtime/bundled-skills/` | Four third-party skill projects, pinned in `SOURCES.json` | MIT each; texts in `licenses/` and embedded in the binary | See [bundled skills](#bundled-skills). |
| Rust crate dependencies | crates.io | Per crate; see `THIRD_PARTY_NOTICES.md` on each release | See [crate licenses](#crate-licenses). |
| npm dependencies `codebase-memory-mcp`, `@playwright/mcp` | npm registry | MIT; Apache-2.0 | Installed next to the npm package, not bundled into it. |
| `assets/` | Claw Code | Not covered by the MIT grant | See [assets](#assets). |
| Launchers, installers, release pipeline, `docs/GATEWAY_CONTRACT.md` | Written for Neuron | MIT, "Copyright (c) 2026 RAHUL S / Zero-X" | |

`LICENSE` keeps the Claw Code copyright line next to Neuron's, as the MIT license
requires, and `NOTICE` records the lineage. Both ship in the npm package and the
Python wheel; the binary embeds the bundled skills' license texts.

## Reference data snapshots

`src/reference_data/` holds JSON inventories used by the porting workspace:

- `commands_snapshot.json` (207 entries) and `tools_snapshot.json` (184 entries): each
  entry is a module name, a `source_hint` path such as `commands/add-dir/add-dir.tsx`,
  and a generated one-line description.
- `archive_surface_snapshot.json`: the file list of an archive root named
  `archive/claude_code_ts_snapshot/src`.
- `subsystems/*.json`: per-directory module counts and sample paths.

These files mirror the names and directory layout of an archived TypeScript source
tree of Anthropic's Claude Code. The repository contains only the names and paths, not
that source: `archive/` is git-ignored, and no commit on any branch adds files under it.
The `neuron` binary does not use these files.

## Bundled skills

| Skill | Repository | Pinned commit | Copyright |
| --- | --- | --- | --- |
| ponytail | <https://github.com/DietrichGebert/ponytail> | `cd76519` | (c) 2026 DietrichGebert |
| antislop | <https://github.com/miqdadbadjuber/anti-slop> | `388cbe3` | (c) 2026 Miqdad Badjuber |
| gstack | <https://github.com/garrytan/gstack> | `857466f` | (c) 2026 Garry Tan |
| agent-reach | <https://github.com/Panniantong/Agent-Reach> | `a19a171` | (c) 2025 Agent Eyes |

All four are MIT. Neuron adds host-adapter sections to some of them; those additions
are Neuron's.

## Crate licenses

For the five release targets, the binary links about 215 third-party crates. Most are
`MIT OR Apache-2.0`. The less common terms, all permissive or weak copyleft:

- `Unicode-3.0`: the ICU4X crates (`icu_*`, `litemap`, `tinystr`, `writeable`, `yoke`,
  `zerovec` and others) and `unicode-ident`.
- `ISC`: `rustls-webpki`, `untrusted`; `ring` is `Apache-2.0 AND ISC`.
- `BSD-3-Clause`: `subtle`. `BSL-1.0`: `clipboard-win`, `error-code`.
- `CDLA-Permissive-2.0`: `webpki-roots` (Mozilla's root certificate data).
- `MPL-2.0`: `option-ext`. File-level copyleft: modified versions of its files must be
  published under MPL-2.0; using it unmodified, as Neuron does, only requires the
  notice.

`clipboard-win` ships no license file; its SPDX text applies. The release workflow
generates the full list and license texts with
`scripts/generate-third-party-notices.mjs`, and Packaging CI fails if a crate has no
license metadata.

## Assets

`assets/` contains images inherited from Claw Code's README: `claw-hero.jpeg`,
`star-history.png`, `tweet-screenshot.png`, `wsj-feature.png`, `sigrid-photo.png` and
`omx/`. No current Neuron document references them. A screenshot of a news feature, a
screenshot of a social media post and a photo of a person are owned by their authors
or subjects, and the MIT license of the code does not cover them.

## Other branches and leftovers

- **Go `opencode` fork.** The `THE-BUGGY-BREACH` branch contains a Go program in `go/`
  (module `github.com/opencode-ai/opencode`, `go/LICENSE`: MIT, "Copyright (c) 2025
  Kujtim Hoxha"). That branch's top-level `LICENSE` names only RAHUL-DevelopeRR and
  drops the Claw Code notice. It is not part of this product line; see
  [ISSUES_TRIAGE.md](ISSUES_TRIAGE.md).
- **`opencode` gitlink.** An orphan submodule entry (commit `73ee493`) with no
  `.gitmodules`; see [CONTRIBUTING.md](../CONTRIBUTING.md#the-opencode-entry).
- **`.claude/sessions/`.** Twelve agent session transcripts (about 21 KB) were committed
  before the path was added to `.gitignore`. A pattern scan found no credentials in
  them.

## Before a paid launch

Have counsel review the following before charging for Neuron or Zero-X plans:

1. **Lineage.** Confirm that the MIT obligations for Claw Code and the bundled skills
   are met by `LICENSE`, `NOTICE` and `THIRD_PARTY_NOTICES.md`, and review how Claw
   Code itself was produced, given that its reference snapshots mirror Claude Code's
   file layout.
2. **Reference snapshots.** Decide whether to keep `src/reference_data/`, which names
   files of a proprietary product, in a commercial repository; nothing in the shipped
   binary depends on it.
3. **Trademarks.** User-facing text and identifiers still contain "Claude" and "Claw"
   (for example the `rusty-claude-cli` crate, `.claude` and `.claw` settings paths and
   some help strings). Review them against Anthropic's and others' trademark guidelines
   and rename user-facing ones.
4. **Assets.** Remove or license the images in `assets/`.
5. **Upstream provider terms.** The free plan depends on Groq, Gemini API and
   Cloudflare Workers AI free tiers permitting this kind of service, and paid plans
   resell upstream capacity; confirm each provider's current terms (the gateway
   repository keeps its analysis in `docs/PROVIDER_TERMS.md`).
6. **Privacy disclosures.** The README tells users that prompts are processed by
   third-party providers and that paid plans use free pools first. Make sure the Zero-X
   terms and privacy policy say the same.
