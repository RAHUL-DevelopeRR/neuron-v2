# Neuron Desktop v1: analysis and build plan

Status: Gate 1 approved 2026-11-06; v1 built and at Gate 2 on 2026-11-07 (section 11.5). Branch `feat/desktop-app`.
Supersedes the desktop parts of `docs/NEURON_APPS_PLAN.md` (that file stays the engine audit).

## 0. Summary

- **What exists.** A working Tauri 2 + React 19 desktop app in `ui/neuron-desktop`, on the shared
  TypeScript client `ui/neuron-client` and the Rust engine (`neuron engine --stdio`, JSON lines).
  It has these pieces:
  - the sidebar, chats, composer, transcript, approvals, the trust gate, sign-in and settings
  - a side pane with changes, files, tasks, extensions and a terminal
  - a release workflow that has already produced macOS and Windows installers from CI
- **What this branch did first.** `feat/desktop-app` merges the two unmerged desktop branches,
  `feat/desktop-shell` (shell, account, settings, first run; PR #16) and `feat/apps-release`
  (installers, apps CI, release docs; PR #14), onto the current engine. There were no conflicts.
- **What is missing for v1.**
  - The desktop speaks an older slice of the engine protocol, so live output, checkpoints,
    commit, reports and session changes never reach it.
  - It runs one chat at a time.
  - It lacks the table stakes users now expect from a coding-agent desktop app (section 4).
  - CI has never produced a Linux artifact, and nothing has been built on the current engine.
- **The plan.**
  1. One foundation slice: the protocol and the reducer.
  2. Six parallel lanes, each in its own git worktree and lane branch, test-first.
  3. Integration, review and a local Windows installer build.
  4. Unsigned CI builds for Windows, macOS and Linux.
- Signing and notarization come later. Android follows as the Expo remote-control app.

## 1. Current state (verified 2026-11-06)

### 1.1 Stack

| Layer | What | Where |
|---|---|---|
| UI | React 19.3, Vite 8, TypeScript 5.9, xterm 6, bun 1.3.14 | `ui/neuron-desktop/src` |
| Shared client | protocol mirror, session reducer, controller, account and gateway problems, diffs | `ui/neuron-client/src` |
| Native host | Tauri 2.9 with dialog, store and updater plugins; portable-pty; git; workspace bounds; recent projects | `ui/neuron-desktop/src-tauri/src/{lib,engine,git,workspace,terminal,projects}.rs` |
| Engine | `neuron engine --stdio`, a sidecar per window; engine protocol v2 with capability flags | `rust/crates/rusty-claude-cli`, `rust/crates/neuron-tui/src/protocol.rs` |
| Release | `desktop-release.yml`, `apps-ci.yml`, `cli-manifest.yml`, `mobile-eas.yml`; `docs/RELEASING.md` | `.github/workflows` |

`src-tauri` is a Cargo project of its own, so the engine workspace and its CI never compile
Tauri.

### 1.2 Desktop features that work today

- **Projects and chats:**
  - a projects page with recent projects
  - chats grouped by day, with search and resume
  - New chat
- **Composer:**
  - `@` file mentions
  - a slash menu of 12 fixed commands
  - menus for mode (read only, edit, full access, plan) and model
  - Stop
- **Transcript:**
  - Markdown and reasoning
  - tool cards with diffs and diffstats
  - question cards
  - sub-agents
  - copy buttons
- **Side pane:**
  - git changes with diffs
  - file tree and preview
  - tasks and sub-agents
  - extensions (MCP servers, plugins, skills, agents)
  - a PTY terminal
- **Approvals:** allow once, allow for this session, deny.
- **Trust:** a trust gate with a preview of what trusting turns on.
- **Account:**
  - Zero-X device sign-in
  - an account menu
  - usage
  - problem cards for gateway errors
- **Settings:**
  - theme, send key, reasoning, folding
  - a folder picker, the settings store and an update check

### 1.3 Release pipeline

The last `desktop-release.yml` run (2026-10-23, PR #14) succeeded and uploaded three artifacts:
`desktop-windows-x64` (6 MB), `desktop-macos-universal` (17 MB) and `desktop-release` (23 MB).
There was no Linux artifact. Section 7 has the audit of triggers, publishing steps and blockers.

## 2. TUI analysis and parity

The TUI is `ui/neuron-tui` (Bun + OpenTUI). Its relevant surface:

- **Input modes:** default, plan, review, interview, skill and report. Bash and image modes are
  hidden behind flags.
- **Composer:**
  - a slash menu built from the engine's `ready.features`
  - `@` mentions
  - prompt history (Up/Down)
  - a queue for prompts sent while a turn runs (`/queue`, Ctrl+Q)
  - pasted images and text
- **Slash commands, UI side:** `/help`, `/new`, `/history`, `/copy`, `/export` (md/json),
  `/model`, `/plan`, `/review`, `/interview`, `/queue`, `/todo`, `/theme:toggle`, `/upgrade`,
  `/skill:*`.
- **Slash commands, engine side:** `/login`, `/logout`, `/status`, `/cost`, `/usage`, `/init`,
  `/memory`, `/doctor`, `/context`, `/compact`, `/permissions`.
- **Git commands:** `/changes`, `/diff`, `/commit`, `/rewind`.
- **Live output:** shell output as it prints, tool-input progress, per-request output progress.
- **Status bar:** model, mode, trust, git branch and counts, tokens, context %, plan and email.
- **Capabilities the TUI declares:** `reports`, `live_progress`, `git_views`, `live_output`. The
  desktop declares only `questions` (`ui/neuron-client/src/controller.ts:67`). The engine
  therefore never sends the desktop live output, reports or git views.

### 2.1 Protocol drift

The engine's events and commands are listed in `rust/crates/neuron-tui/src/protocol.rs:94-165`
and `:492-813`.

**Engine events the shared client does not model:**
- `report`
- `tool_input_started`, `tool_input_progress`
- `output_progress`, `request_usage`
- `tool_output_delta`
- `changes`, `diff`, `commit_proposal`
- `checkpoints`, `rewound`

**Commands it cannot send:** `commit`, `rewind`, `set_mcp_server_enabled`.

### 2.2 Parity table (desktop vs TUI)

| Feature | Desktop today | v1 action |
|---|---|---|
| `@` file mentions | present | add `@folder` |
| Image paste and drop | missing | add, gated on `model_images` |
| Prompt history (Up/Down) | missing | add |
| Queue prompts during a turn | missing (Send becomes Stop) | add |
| Slash menu | 12 fixed entries | build from engine features, all commands |
| Report panels (`/usage` `/init` `/memory` `/doctor`) | missing | add |
| Live shell output, tool-input progress | missing | add |
| Per-request tokens and output progress | missing | status bar |
| Session changes and diffs from the engine | git-only | engine `changes`/`diff`, git as fallback |
| Commit with an editable message | missing | commit card |
| Rewind and checkpoints | missing | rewind picker (dry run, modes) |
| MCP server on/off | list only | toggle (`mcp_toggle`) |
| Reasoning effort | missing | picker, passed to the engine |
| `/copy`, `/export` | per-reply copy only | whole chat, md/json |
| Permissions, model, plan mode, questions, todos, sub-agents, trust, sign-in, history, projects | present | keep |
| Terminal, file tree, settings page | desktop-only extras | keep |

## 3. Competitive analysis

Surveyed on 2026-11-06 from vendor docs and secondary coverage.

- Rows marked `*` come from prior knowledge and were not re-checked.
- Sources include
  [Codex on Windows](https://developers.openai.com/codex/windows),
  [Claude Code desktop](https://code.claude.com/docs/en/desktop),
  [Zed ACP registry](https://zed.dev/blog/acp-registry), and
  coverage of OpenCode 2.0, Cursor 3.x and Windsurf 2.0.

| | Codex app | Claude Code desktop | OpenCode | Cursor 3 | Windsurf | Zed |
|---|---|---|---|---|---|---|
| Parallel sessions + worktrees | yes, worktrees | yes, `.claude/worktrees` | background agents | `/worktree`, best-of-N | parallel Cascade | threads* |
| Attention routing | notifications* | "needs you" sidebar | ? | notifications* | yes* | yes* |
| Diff review, undo | review pane* | diff review, rewind* | `/undo` `/redo`* | checkpoints* | checkpoints* | hunk review* |
| Approvals, sandbox | presets; native Windows sandbox | modes incl. plan and auto* | allow/ask/deny* | allowlist* | levels* | profiles* |
| Git, PR | built in; PRs* | PR monitoring | GitHub agent* | PRs* | basic* | git panel* |
| MCP, skills, plugins | skills, plugins | connectors, plugins, skills | plugins, MCP* | team MCPs | MCP store* | ACP + MCP |
| Cloud, phone | cloud tasks*; mobile | web, cloud, SSH, phone dispatch | `serve` / `attach` | cloud + SSH; iOS | ? | SSH* |
| Framework, OS | Electron*; macOS + Windows | Electron*; macOS + Windows* | Tauri (secondary); 3 OS* | Electron fork; 3 OS | Electron fork; 3 OS | native Rust; 3 OS |

**What this means for Neuron:**

- **Linux is uncovered.** Two of the leaders, Codex and Claude Code desktop, do not ship on Linux.
  A Linux build of equal quality is a real differentiator.
- **The same table stakes appear in every product.**
  - parallel sessions with worktree isolation
  - a "needs you" queue with OS notifications
  - reviewable, revertible changes
- **Neuron already has the hard parts.** It has a stable engine protocol, approvals, sessions,
  checkpoints in the engine, and a terminal. The gap is mostly UI wiring.

**Table stakes, ranked.** Items marked v1 are in this plan; `→` names the lane.

1. Threads per project that persist and are searchable. **Have.**
2. A live, interruptible transcript with streaming shell output. **v1 → L1.**
3. Inline approvals, visible modes and scoped "always allow". **Have.** Deny-with-reason and
   keyboard approve/deny come in L3.
4. Diff review with per-file revert, plus checkpoint restore. **v1 → L2.** Per-hunk revert is v1.1.
5. Parallel sessions with one-click worktree isolation and cleanup. **v1 → L4.**
6. Attention routing: OS notifications, a dock or taskbar badge, a "needs you" group.
   **v1 → L4.**
7. Git basics: status, commit, open in editor. **v1 → L2.** Create PR through `gh` is a stretch
   goal.
8. Model and reasoning effort, a context meter, usage and quota. **v1 → L1 and L3.**
9. Plan mode with a live todo checklist next to the composer. **Have the data; v1 → L1 shows it.**
10. MCP manager. **v1:** list and enable or disable (L5). Adding a server and signing in need
    engine work (E3), so they are v1.1.
11. A terminal per session's working directory. **Have.** v1 → L4 makes it follow the chat's
    worktree.
12. Composer basics: `@` mentions, slash commands, images and files. **v1 → L3.**
13. Command palette and hotkeys. **v1 → L4.**
14. Onboarding: sign-in, recent folders, trust. **Have.**
15. Signed auto-update. **Signing phase.** v1 fixes the updater wiring (L6).

**UX patterns adopted (described in our own words):**
- **Triage sidebar.** Chats are grouped as Needs you, Running, Done-unread and Idle. A badge jumps
  to the waiting prompt.
- **Permission-mode chip in the composer.** Shift+Tab cycles it, and risky modes are tinted.
- **Worktree chosen at chat start.** The chat header shows the branch, Open in editor and Discard.
- **Changes pane with two scopes:** "this session" (the engine's changes) and "this folder"
  (git), with a revert per file.
- **Restore points.** The rewind picker shows a dry-run preview of what would change and what
  cannot be restored.
- **Quiet tool cards.**
  - Reads collapse to one line ("Read 6 files").
  - Shell commands open into live output, with the exit code and duration.
- **A status-bar meter** for context %, session tokens and cost, and quota, which warns before a
  limit is hit.

## 4. Desktop v1 scope

**v1 (this build):** everything in section 2.2, plus table-stakes items 2–13 above, to the extent
the lanes in section 6 list them, plus unsigned installers for Windows, macOS and Linux.

**v1.1 (next):**
- per-hunk revert, and comments on the diff that become the agent's next instruction
- Create PR, CI status and auto-merge through `gh`
- a "rewind to here" button on each user message, and forking a chat
- MCP add, sign-in and remove; provider keys in the keychain (engine phases E3 and E7)
- an in-app browser preview
- best-of-N: several models on one task, in separate worktrees, with their diffs compared

**Later:**
- an engine server the UI attaches to (local, SSH, cloud) and "Continue on phone" (phase R1)
- the Android companion (section 8)
- ACP support, scheduled routines, voice input, a plugin marketplace
- signing, notarization and a signed updater channel (the signing phase)

## 5. Architecture

```
┌─────────────────────────────── Neuron Desktop (Tauri 2) ────────────────────────────────┐
│ WebView (WebView2 / WKWebView / WebKitGTK): React 19 UI, ui/neuron-desktop/src           │
│   sidebar · chat list with live status · transcript · composer · side pane · palette     │
│   ui/neuron-client: protocol.ts (mirror of protocol.rs) · session.ts reducer · controller │
│        ▲ Tauri events per chat                       │ invoke(engine_send, chat, msg)     │
│ Rust host, src-tauri                                 ▼                                   │
│   engine.rs: one `neuron engine --stdio` sidecar per open chat (registry, caps, restart)  │
│   workspace.rs (bounded fs, search) · git.rs (status, diff, worktrees) · terminal.rs     │
│   plugins: dialog · store · updater · notification (new)                                  │
└───────────────┬─────────────────────────────────────────────────────────────────────────┘
                │ stdin/stdout JSON lines (engine protocol v2); no port is opened
        neuron engine --stdio   (bundled as binaries/neuron-<target-triple>)
                │ HTTPS
        Zero-X gateway, or the user's own provider
```

### 5.1 Decisions

1. **Keep Tauri 2.** It is already built and tested. The installers are small (the Windows
   installer was 6 MB plus the engine). The webview is native on every OS, the host is in Rust
   like the engine, and Tauri 2 also targets Android and iOS if that is ever wanted.
2. **The engine stays a sidecar process, not a linked library.**
   - It is the same binary the CLI ships, and it speaks the same protocol as the TUI and mobile.
   - It is crash-isolated and can be updated independently.
   - The cost is the Windows Defender issue in section 9.
3. **One engine per open chat.**
   - This lets several chats run at once.
   - Events are namespaced by chat id. A background chat keeps running, and its approvals raise
     a sidebar badge and an OS notification.
   - A cap of 4 running engines applies. Idle engines shut down, and the chat resumes on open
     with `--resume`.
4. **Optional worktree per chat.**
   - "New chat in a worktree" runs `git worktree add` on a `neuron/<slug>` branch and starts the
     engine there.
   - The Changes pane shows that worktree.
   - Merging back is a v1.1 action.
5. **Capabilities are declared by the front end.** The controller takes a capability list. The
   desktop declares `questions`, `reports`, `live_progress`, `live_output` and `git_views`.
6. **No engine (Rust) changes in v1.** Everything the desktop needs already exists in the
   engine. Engine gaps go to the v1.1 list.

## 6. Work breakdown

The process follows the ECC `orch-add-feature` pipeline:

1. Research, then this plan (Gate 1).
2. Test-first lanes.
3. Review.
4. Commit (Gate 2).

**How lanes work:**
- Each lane is a subagent working in its own worktree, `../neuron-desk-<lane>`, on a local branch
  `lane/desk-<lane>`.
- Nothing is pushed or merged into `feat/desktop-app` before Gate 2.
- Rust builds share one `CARGO_TARGET_DIR`, which reuses the 1.8 GB Tauri cache of the merged
  `feat/desktop-shell` worktree, because disk is tight.

| Lane | Delivers | Owns (edits elsewhere stay additive) | Depends on |
|---|---|---|---|
| **L0 Foundation** | the protocol mirror synced with `protocol.rs` (events and commands from section 2.1); reducer state for live output, progress, changes, commit proposals, checkpoints and reports; capability list in the controller; a stronger drift test | `ui/neuron-client/src/{protocol,session,controller,index}.ts`, `ui/neuron-client/test/*` | none |
| **L1 Live transcript** | live shell output in tool cards, with exit code and duration; tool-input progress; reads collapsed into one line; a status-bar meter (context %, tokens and cost, quota warning); report panels (`/usage` `/init` `/memory` `/doctor`); a todo strip pinned under the transcript | `Transcript.tsx`, `Bars.tsx`, new `LiveOutput.tsx`, `ReportPanel.tsx`, `TodoStrip.tsx` | L0 |
| **L2 Review and git** | Changes with two scopes, the engine session (`changes`/`diff`) and the folder (git); revert per file, with confirmation; commit card with an editable message; rewind picker with dry run and not-restored files; Open in editor | `SidePane.tsx`, new `CommitCard.tsx`, `RewindPicker.tsx`, `src-tauri/src/git.rs` | L0 |
| **L3 Composer** | slash registry from engine features; image paste and drop (`model_images`); `@folder`; prompt history; queue during a turn; reasoning-effort picker; permission-mode chip cycled with Shift+Tab; approvals with Enter/Esc and deny-with-reason; `/copy` and `/export` | `Composer.tsx`, `PermissionDialog.tsx`, new `src/commands.ts`, `src-tauri/src/workspace.rs` (staging, save) | L0 |
| **L4 Multi-chat** | engine registry (one per chat, cap 4, idle shutdown, resume); chats run in the background; triage sidebar (Needs you, Running, Done-unread, Idle); OS notifications and a dock/taskbar badge; new chat in a worktree (new `worktree.rs`) with a chat header (branch, Open in editor, Discard); terminal follows the chat's folder; command palette (Ctrl/Cmd+K); shortcuts sheet | `App.tsx`, `Sidebar.tsx`, `src-tauri/src/{engine,lib,worktree}.rs`, new `CommandPalette.tsx`, `ChatHeader.tsx` | L0 |
| **L5 Extensions and resilience** | MCP on/off toggle; which engine is running (bundled, `NEURON_BIN` or PATH) and its version; problem cards for a missing or quarantined engine (Defender, Gatekeeper), spawn errors, a crash, a failed restart; diagnostics copy | `ExtensionsPanel.tsx`, `ProblemCard.tsx`, `SettingsPage.tsx`, new `src-tauri/src/diagnostics.rs` | L0 |
| **L6 Packaging and CI** | artifacts-only unsigned builds for Windows (NSIS, MSI), macOS (universal dmg) and Linux (AppImage, deb, rpm) with the engine inside; Linux in the default platform set; updater endpoint fix (section 7.5); Tauri pin; unsigned-install notes; engine handshake check | `.github/workflows/{desktop-release,apps-ci}.yml`, `src-tauri/{Cargo.toml,tauri*.conf.json}`, `vite.config.ts`, `docs/RELEASING.md`, `ui/neuron-desktop/README.md` | none (starts with L0) |

**Every lane:**
- writes failing tests first (`bun test` with happy-dom, plus `cargo test` for `src-tauri`)
- keeps `bun test` and `bun run typecheck` green in `ui/neuron-client` and `ui/neuron-desktop`
- runs `cargo fmt`, `cargo clippy --all-targets -D warnings` and `cargo test` in `src-tauri` when
  it touches Rust
- makes conventional commits on its lane branch

**After the lanes:**
1. Merge them into `lane/desk-integration` and resolve conflicts (`App.tsx` and `lib.rs` are the
   hotspots).
2. Review: TypeScript reviewer, Rust reviewer and `security-reviewer`. File system, PTY,
   process spawn and git all trigger it.
3. Fix CRITICAL and HIGH findings.
4. Build the Windows installer locally and smoke-launch it.
5. Gate 2.

## 7. Build and release

Audited on 2026-11-06 at `956bfe4`. No build or workflow was run for the audit.

### 7.1 Baseline

- **`ui/neuron-desktop`:** 55 pass, 3 skipped (engine e2e). Typecheck and `vite build` pass.
- **`ui/neuron-client`:** 79 pass, 9 skipped, **4 fail**. All four are the protocol drift test:
  `protocol.ts` is behind `protocol.rs` (section 2.1).
  - This turns Apps CI red on every PR.
  - L0 fixes it.
- **Lockfiles and versions:** `--locked` holds for `rust/` and `src-tauri`.
  - Tauri crate 2.12.5 vs CLI/API 2.12.4, which is the same minor.
  - Plugins pinned identically in Cargo and npm: dialog 2.8.1, store 2.5.0, updater 2.13.2.

### 7.2 How the engine gets into the installer

The `desktop-release.yml` build job builds `cargo build --release --locked -p rusty-claude-cli
--target <triple>` and stages the result as `src-tauri/target/sidecar/neuron-<triple>[.exe]`.

- **macOS:** one universal binary, made with `lipo`.
- **Release overlay:** `tauri.release.conf.json` adds `externalBin`.
  - Only the overlay ships the engine.
  - A plain `tauri build` ships without it.
- **Runtime lookup** (`src-tauri/src/engine.rs:18-40`):
  1. `NEURON_BIN`
  2. a `neuron` next to the app's executable
  3. `PATH`
- **Gap (L5):** if the bundled copy is missing (for example quarantined), the app silently uses
  whatever `neuron` is on PATH, possibly another version. The app must say which engine it runs
  and warn when it isn't the bundled one.

### 7.3 Triggers, and what can leave the building

**Triggers:**
- a `desktop-v*` tag push
- `workflow_dispatch` with inputs `platforms` (`alpha` | `all`) and `dry_run` (default true)
- PRs touching the workflow or the release config

**Outward-facing steps and their gates:**

| Step | What it does | Gate |
|---|---|---|
| `release` | a **draft** GitHub release with the installers, checksums and manifest | a `desktop-v*` tag AND (a push, or a dispatch with dry_run=false) |
| `publish` | makes the draft public | every build signed and macOS notarized, plus the `desktop-release` environment |
| `manifest` | writes `releases/desktop/latest.json` and `updater.json` to R2 (zero-x.live/releases) | `publish` succeeded, plus the `release-manifests` environment and R2 secrets |
| artifacts | `desktop-<platform>` workflow artifacts | always. The repo is **public**, so any signed-in GitHub user can download them |

- **A dispatch from a branch is artifacts-only, whatever the inputs.**
- **No secrets exist in the repo today, so builds degrade cleanly:**
  - Windows is unsigned.
  - macOS is ad-hoc signed and not notarized.
  - No updater artifacts are made.
  - `publishable=false`.
- **Before any tag push:** the `desktop-release` and `release-manifests` environments do not exist
  yet. GitHub would create them without reviewers, so the approval step in `docs/RELEASING.md`
  would not be enforced. Create both with reviewers first.

### 7.4 v1 build targets (unsigned)

| OS | Artifacts | Runner | Status |
|---|---|---|---|
| Windows x64 | NSIS `-setup.exe` (per-user), MSI | windows-2022 | built green on 2026-10-23, before the shell merge |
| Windows arm64 | NSIS | cross-compiled on windows-2022, tested on windows-11-arm | never run |
| macOS | universal `.dmg` / `.app` (arm64 + x64) | macos | built green on 2026-10-23 |
| Linux x64 | AppImage, `.deb`, `.rpm` | ubuntu-22.04 | never run (`platforms=all` only) |

Command, after Gate 2 and the push:

```
gh workflow run desktop-release.yml --ref feat/desktop-app -f platforms=all -f dry_run=true
```

### 7.5 Fixes in L6

1. **Updater endpoint.**
   - It points at `latest.json`, the site manifest, which has no `url` or `signature`. `check()`
     always fails, and the error is swallowed (`platform/tauri.ts:184-192`).
   - Point it at `updater.json`.
   - With no `pubkey`, show "a new version is on the releases page" instead of a silent failure.
2. **Keep `TAURI_SIGNING_PRIVATE_KEY` unset until a real `pubkey` exists.** With a key and an empty
   `pubkey`, every build fails at "failed to decode pubkey".
3. **Pin the Tauri version.** Change `tauri = "2.9"` / `tauri-build = "2.5"` to `~2.12`, so a
   `cargo update` cannot trip the CLI's version-mismatch check.
4. **`vite.config.ts:18`:** use `import.meta.dirname` instead of `__dirname`.
5. **Unsigned-install notes,** per OS, in `ui/neuron-desktop/README.md` and `docs/RELEASING.md`.
6. **Engine check:** confirm locally that a signed-out `neuron engine --stdio` prints `ready` first.
   The install-test job assumes it does.

### 7.6 Local Windows installer (fits in about 1 GB)

```powershell
$s='ui\neuron-desktop\src-tauri\target\sidecar'; New-Item -ItemType Directory -Force $s
Copy-Item rust\target\debug\neuron.exe "$s\neuron-x86_64-pc-windows-msvc.exe"
$env:CARGO_INCREMENTAL='0'; cd ui\neuron-desktop
bun run tauri build --config src-tauri/tauri.release.conf.json --bundles nsis --debug
# -> src-tauri\target\debug\bundle\nsis\Neuron_0.1.0_x64-setup.exe
```

- A release build (LTO) needs about 2 GB more.
- The first bundle downloads NSIS.

### 7.7 Unsigned first run, until the signing phase

| OS | What the user sees | What to do |
|---|---|---|
| Windows | SmartScreen "Windows protected your PC" | More info → Run anyway |
| Windows | Defender may quarantine `neuron.exe` | the L5 problem card explains it |
| macOS | "cannot be opened because the developer cannot be verified" | System Settings → Privacy & Security → Open Anyway (macOS 15 removed the right-click → Open bypass), or `xattr -dr com.apple.quarantine /Applications/Neuron.app` |
| Linux | AppImage won't run | `chmod +x Neuron_*.AppImage`; the `.deb`/`.rpm` install normally |

## 8. Android (after the desktop)

**Recommendation: continue the Expo (React Native) remote-control app** on
`feat/mobile-alpha-build`. Do not build a Tauri Android port of the desktop UI.

- **Why not Tauri on Android.**
  - The desktop host spawns the engine, a PTY and git on the local machine. A phone cannot run
    the engine's shell tools against a workspace.
  - A Tauri Android build would still need a remote engine, which is the Expo design on a
    heavier stack, without a touch-first UI.
- **What exists.**
  - **App:** Expo SDK 58 / React Native 0.88. Sessions, the transcript, a permission sheet with
    diff preview and deny-with-note, questions, todos, send and stop, QR pairing and Zero-X
    sign-in.
  - **Relay:** an end-to-end-encrypted relay protocol (X25519 + HKDF, AES-256-GCM, fixed test
    vectors in TypeScript and Rust).
  - **Tests:** 12 `bun test` files.
  - **Build:** an account-free APK build (`expo prebuild` + Gradle, workflow
    `mobile-native.yml`). The computer side, `neuron engine --share`, is on `feat/engine-share`.
- **Order of work.**
  1. Merge `feat/engine-share`. It is Rust-only.
  2. Merge `feat/mobile-alpha-build`. Expect conflicts in `ui/neuron-client`
     (`index.ts`, `session.ts`, `protocol.ts`, `controller.ts`) and in the mobile skeleton.
     Resolve by taking the union and regenerating the lockfiles.
  3. Make the engine serve remote protocol 1 itself, so the desktop gets "Continue on phone"
     without the Bun host.
  4. Deploy the zero-x.live `RelayRoom` and add the relay section to `docs/GATEWAY_CONTRACT.md`.
  5. Produce a debug-signed APK from CI, then an upload keystore, an AAB and an emulator smoke
     test.
  6. Push notifications for approvals (FCM and an EAS project, which are account-bound).

## 9. Risks

| Risk | Mitigation |
|---|---|
| **Defender quarantines the unsigned `neuron.exe`** (seen 2026-11-04 as `Bearfoos.A!ml`), so the sidecar fails to spawn | L5 problem card that names the cause and the recovery; Authenticode signing (for example Azure Trusted Signing) in the signing phase; a false-positive submission |
| **Unsigned installers warn on first run** (SmartScreen, Gatekeeper) | Documented steps: "More info → Run anyway"; right-click → Open or `xattr -dr com.apple.quarantine`; `chmod +x` for the AppImage |
| **Disk: C: has 4.9 GB free** | Shared `CARGO_TARGET_DIR`, debug Rust builds in lanes, one release build at the end. Two stale Tauri caches (4.5 GB) can be reclaimed with approval |
| **`App.tsx` and `lib.rs` merge conflicts** between lanes | Ownership table above, additive edits, sequential merge by the orchestrator |
| **WebKitGTK differences on Linux** | The Linux CI job builds; a Linux smoke test comes after v1 |
| **Protocol drift between Rust and TS** | The L0 drift test parses `protocol.rs` for every event and command variant |

## 10. Decisions for Gate 1 (answered 2026-11-06)

The owner chose all lanes including L4, approved deleting the cache (done), and approved the early
CI run (run 42259399488: Windows x64, Windows arm64 and Linux x64 built and passed the install
test; macOS was cancelled because `macos-14` runners no longer run, so the desktop jobs moved to
`macos-15`).

1. **The plan.** Approve it and start all lanes, or approve without L4 (multi-chat) to finish
   sooner.
2. **Disk.** Delete the stale Tauri cache `neuron-release-wt/ui/neuron-desktop/src-tauri/target`
   (2.7 GB)? Its branch is merged here and it can be rebuilt.
3. **Early CI.** Run the artifacts-only build for all platforms now:
   `gh workflow run desktop-release.yml --ref feat/desktop-app -f platforms=all -f dry_run=true`.
   The repo is public, so signed-in GitHub users can download the artifacts. Nothing is released.

## 11. Progress and context (handoff, 2026-11-06)

### 11.1 Done

- **Branch `feat/desktop-app`**, from `feat/quota-refresh-continuity` @ `d6fd041`:
  - `f000d96`: merge of `feat/desktop-shell` (the content of PR #16), no conflicts
  - `956bfe4`: merge of `feat/apps-release` (the content of PR #14), no conflicts
  - the next commit: this plan, plus a pointer entry in `PROGRESS.md`
- **Phase 1 of the ECC `orch-add-feature` pipeline (research)**, size "large", by four subagents:
  - TUI parity: section 2
  - competitors: section 3
  - the build and release audit: section 7
  - Android: section 8
- **Phase 2 (plan):** this document. **Gate 1 is open** (section 10). No implementation code has
  been written yet.
- **Test baseline:**
  - `ui/neuron-client`: 79 pass, 9 skipped, 4 fail (protocol drift, which L0 fixes).
  - `ui/neuron-desktop`: 55 pass, 3 skipped. Typecheck and `vite build` pass.

### 11.2 How to resume

1. **Get Gate 1 answers** (section 10).
2. **Start L0 and L6 together.**
   - L6 branches from `feat/desktop-app`.
   - L0 creates `lane/desk-foundation`.
3. **Start L1–L5**, each with
   `git worktree add "C:/RAHUL/PROJECTS _OF_Rahul/neuron-desk-<lane>" -b lane/desk-<lane> lane/desk-foundation`.
   - Each lane works test-first and commits only to its lane branch.
   - No lane pushes.
4. **Merge** the lanes into `lane/desk-integration`. The conflict hotspots are `App.tsx`,
   `lib.rs`, `engine.rs`, `platform/*`, `styles.css` and `test/fake-platform.ts`.
5. **Review:** TypeScript reviewer, Rust reviewer and `security-reviewer`. Fix CRITICAL and HIGH
   findings.
6. **Build the local Windows installer** (section 7.6) and smoke-launch it.
7. **Gate 2:** confirm the commits, then fast-forward `feat/desktop-app`, push, and run CI
   (section 7.4).

### 11.3 Environment facts

- **Disk:** C: has about 5 GB free. Rust caches, in GB:

  | Cache | Size | Notes |
  |---|---|---|
  | `neuron-live-wt/rust/target` | 12.0 | debug only; the engine is at `rust/target/debug/neuron.exe` |
  | `neuron-clawport-wt` | 10.5 | worktree touched in the last 2 days; leave it |
  | `neuron-web-wt` | 7.7 | worktree touched in the last 2 days; leave it |
  | `neuron-colony-wt` | 7.3 | worktree touched in the last 2 days; leave it |
  | `neuron-alpha3-wt` | 5.3 | worktree touched in the last 2 days; leave it |
  | `neuron-release-wt` Tauri | 2.7 | branch merged here; 2 weeks stale |
  | `neuron-apps-wt` Tauri | 1.8 | branch merged here; 2 weeks stale |

- **Shared Rust target for `src-tauri` in lanes:**
  `C:/RAHUL/PROJECTS _OF_Rahul/neuron-apps-wt/ui/neuron-desktop/src-tauri/target`. Its
  `Cargo.lock` matches this branch. Use debug builds only, with `CARGO_INCREMENTAL=0`.
- **Engine for e2e tests:** `NEURON_BIN=rust/target/debug/neuron.exe`.
  `mock-anthropic-service` is not built, so the engine e2e tests are skipped.
- **GitHub:**
  - The repo is **public**, and its default branch is `ux-audit-fixes`.
  - Pushing `feat/desktop-app` triggers no workflow.
  - "Desktop release" is registered, so `gh workflow run --ref` works from this branch.
  - The repo has no secrets, so builds come out unsigned (macOS ad-hoc).
- **Open PRs:** #14 and #16 are superseded by this branch. #15 (mobile) belongs to the Android
  phase.
- **Unmerged branches to know about:**
  - `feat/engine-share` (2 commits) and `feat/mobile-alpha-build` (4 commits): Android phase
  - `feat/web-browser` (88 commits): engine browser work, active in another worktree; leave it
- **Defender:** an unsigned `neuron.exe` has been quarantined before
  (report: `docs/INVESTIGATION_LAG_NPM_PATH.md` on `fix/lag-npm-path`).

### 11.4 Code anchors

- **Desktop capabilities:** `ui/neuron-client/src/controller.ts:67` declares only `questions`.
- **Shared client protocol:** `ui/neuron-client/src/protocol.ts`.
  - The UiEvent union is at `:488-515`; the commands at `:557-598`.
  - The drift test is `test/protocol.test.ts`.
- **Engine protocol:** `rust/crates/neuron-tui/src/protocol.rs`.
  - Features at `:94-165`, capabilities at `:217-231`, events at `:492-813`.
- **TUI:**
  - `ui/neuron-tui/src/data/slash-commands.ts:47-261`
  - `src/utils/keyboard-actions.ts`
  - `src/engine/capabilities.ts:31-36`
  - `src/engine/protocol.ts`
- **Engine lookup:** `ui/neuron-desktop/src-tauri/src/engine.rs:18-40`, in the order
  `NEURON_BIN`, then next to the exe, then PATH.
- **Updater:** the error is swallowed at `ui/neuron-desktop/src/platform/tauri.ts:184-192`, and
  the endpoint is at `tauri.conf.json:30`.
- **Release gates** in `.github/workflows/desktop-release.yml`:
  - `is_release` at `:100-104`
  - `publishable` at `:775`
  - release, publish and manifest at `:806-931`
- **Engine sidecar staging:** `desktop-release.yml:248-275`, plus the release overlay
  `src-tauri/tauri.release.conf.json`.

### 11.5 Status at Gate 2 (2026-11-07)

- **Built.** Lanes L0 to L6 were built test-first in `neuron-desk-<lane>` worktrees and merged in
  `lane/desk-integration`, which fast-forwards `feat/desktop-app`. L4 follows a blueprint (engine
  registry in `src-tauri/src/registry.rs`, chat commands in `chats.rs`, worktree copies in
  `worktree.rs`, the chats store in `src/chats.ts`, per-chat IPC in `src/platform/chat-host.ts`).
  No engine (`rust/`) changes.
- **Checks.** `ui/neuron-client` 168 pass; `ui/neuron-desktop` 342 pass (3 real-engine e2e
  skips), typecheck and vite build clean; `src-tauri` fmt, clippy and 100 tests clean; brand lint
  clean. The test scripts run `bun test --timeout 20000`, because axe-heavy UI tests pass 5 s on a
  loaded machine.
- **Reviews.** TypeScript, Rust and security reviewers in three rounds, then a verification pass:
  no CRITICAL; every HIGH fixed and verified in the code.
- **Local installer and smoke test.** The debug NSIS installer (plan 7.6, 11.4 MB) installed per
  user and passed a smoke test against a real `neuron engine --stdio`: the sign-in screen (an
  engine with `NEURON_CREDENTIAL_STORE=file` and temporary `NEURON_HOME`/`NEURON_CONFIG_HOME`
  starts signed out without touching the keychain login), the trust gate, a chat that runs tools,
  an approval, both Changes scopes, two chats at once, and a clean shutdown of every engine. Drive
  the installed app through WebView2's DevTools port
  (`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`); `NEURON_WORKSPACE`
  picks the folder it opens. Do not install into `%LOCALAPPDATA%\Neuron`: another program uses it.
- **Known issues for v1.1.**
  - The engine lists image attachments by path; it does not send images to the model yet.
  - A failed chat restart emits no exit for the old engine.
  - Quitting while a chat starts can orphan an engine on macOS and Linux.
  - A full engine input queue can drop an approval reply.
  - Read-only mode refuses changes instead of asking (the UI now says so).
- **Next (owner).** Fast-forward and push `feat/desktop-app`, dispatch the artifacts-only CI to
  verify `macos-15`, the rpm bundle and Windows arm64, and decide on cleanup of the lane worktrees
  and branches.
