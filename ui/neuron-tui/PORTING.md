# Porting notes: Freebuff CLI → neuron-tui

This package is Freebuff's terminal UI (the `cli/` package of Freebuff at commit
`545e04d`, Apache-2.0) with every Codebuff service taken out, so that Neuron's
Rust engine can drive it. This file records what was removed and why, what
replaced it, and what is left for later milestones.

## Provenance and licence

- **Baseline:** commit `ac2b347` on `feat/tui-freebuff-port` is a verbatim copy
  of Freebuff's `cli/` with its `LICENSE` and `NOTICE`. The P1 commit after it
  holds every change, so `git diff ac2b347 -- ui/neuron-tui` shows exactly what
  was modified.
- **`common/`:** 31 files of Freebuff's `common/src` are vendored under
  `src/vendor/common/`. They are still imported as `@codebuff/common/*`, a
  `tsconfig.json` path alias, so the copied UI code reads as it did upstream.
- **Notices:** `LICENSE` (Apache-2.0) and `NOTICE` are kept unchanged. Every file
  changed from its Freebuff original starts with a "Derived from
  Codebuff/Freebuff (Apache-2.0); modified for Neuron" comment.
  `package.json` cannot hold a comment: it is modified too (new name, Codebuff
  packages removed, exact versions). Files copied unchanged carry no header; the
  package's `LICENSE` and `NOTICE` cover them.
- **Size:** against the baseline, 315 files were deleted (168 source, 147
  tests), 108 modified and 42 added (31 vendored, 11 new). `chat.tsx` went
  from 2,478 to about 1,400 lines.

## What was removed, and why

| Area | What went | Why |
|---|---|---|
| Codebuff SDK | Every `@codebuff/sdk` import. `utils/codebuff-client.ts` and `create-run-config.ts`, and the SDK `client.run()` call in `hooks/use-send-message.ts`, with its BYOK pinning, retry and catalog-retry logic. The Windows terminal-command broker (`utils/terminal-command-broker.ts`, `smoke/`, `/diagnostics`). `native/` (bundled ripgrep). `pre-init/tree-sitter-wasm.ts` and the `--smoke-*` flags. | The SDK ran the agent loop and its tools inside the UI. Neuron's engine runs them; the UI only renders what `neuron engine --stdio` reports. |
| Agents, skills, agent store | Bundled agents (`agents/`, `scripts/prebuild-agents.ts`, `test/setup-agents-artifact.ts`). `.agents/` loading and its trust gate (`agent-dir-trust.ts`, `--trust-agents`), mcp.json. Agent validation before each send. `--agent`, `/publish` (store, mutation, screens). `/init`, which wrote Codebuff's `knowledge.md` and the `.agents/types` templates. | Codebuff's agent system and agent store. The engine owns Neuron's agents and skills. `utils/local-agent-registry.ts` and `utils/skill-registry.ts` remain as empty registries until the engine reports them. |
| Ads | `src/ads/` (10 files). Display, inline and response ad cards, the sponsor dock and its panel, partner rows (`ad-banner`, `partner-ad-line`, `use-gravity-ad`, `use-dock-panel`, `use-partner-suggestion`, `use-composer-intent`). Ad positions and lazy response ads, the ad client identity, the ad trace context in the chat store, and the `allowInlineAds` marker on messages. `/ads:*`. The `gravity_index` tool renderer and the Gravity link references in `render_ui`. Every `noteAd*` call (event handlers, message queue, composer, router), and the ad-rotation activity tracking. | Codebuff's ad network. |
| Sponsored proposals and runs | `utils/sponsored-*` (17 files), the sponsored-run store and proposal hook, the proposal block and dock, and the account prompt. The sponsored turn in the chat runtime and the send path. The sponsored project-identity marker that `setProjectRoot` wrote into every Git project. The profile/sponsored survey box, its store, machine and keys. | Advertiser-run tasks and surveys: Codebuff services. |
| Free-mode waiting room and session queue | The Freebuff session: store, API, display, identity, relaunch, instance owner, device key, chat admission and capacity wait. The model catalog, store, directory and the 1,900-line model selector. Streak, referral, premium reset and Freebucks. The Freebuff chat header, footer, controls, session summary and paused-plan notice. `/model`, `/reasoning`, `/end-session`, `/dashboard`. The Freebuff picks in settings. `IS_FREEBUFF` is pinned to `false` (see below). | Freebuff's hosted free-capacity service. Neuron's model picker comes from the engine in P3. The message queue for prompts sent during a turn is kept: it is UI, not the waiting room. |
| Codebuff login, credits and billing | `login/` (5 files): login flow, plain login, Discord invite. The login modal, auth query and state hooks, login polling, the keychain token store, `utils/auth.ts`, `utils/codebuff-api.ts` and the fingerprint. Usage and subscription queries, monitors and banners. The out-of-credits and subscription-limit input modes. Credits in the message footer and run completion. The Codebuff credit, free-mode, rate-limit and session-gate error classifiers (`utils/error-handling.ts`). `/login`, `/logout`, `/usage`, `/subscribe`, and the `login` and `publish` CLI commands. The post-login Discord banner. | Sign-in comes from the engine (device code, P3), and tokens stay in Rust. |
| BYOK | `utils/byok.ts`, `/byok`, the BYOK connection store, BYOK pinning in the send path and settings, and the BYOK label in the status bar. | Providers and keys are configured in the engine. |
| Telemetry | PostHog analytics and all 24 `trackEvent` calls. The Axiom log shipper and the logger's analytics mirroring. The engagement heartbeat, Windows helper-process census and terminal-health reports, client-environment probe, fatal-crash report, persistent anonymous id, input-typing profile, survey events, SDK trace writer and AI-SDK warning logger. Feedback (`/feedback`, its store and components): it was sent to Codebuff. The Codebuff backend health poller (`connection-monitor`). | Codebuff telemetry. The logger now writes only to a local file: the chat's `log.jsonl`, or `<project>/debug/cli.jsonl` with `NEURON_TUI_ENV=dev`. |
| Release tooling | `release/`, `release-core/`, `release-staging/` (the npm launcher and its packages), `scripts/build-binary.ts`, `release.ts`, `smoke-binary.ts`, `smoke-terminal-broker-windows.ts`, `test-published-self-update.ts`, `test-sdk-file-hooks.sh`, `validate-cli-with-tmux.sh`. | Codebuff's release pipeline. It built the SDK, baked Codebuff's `NEXT_PUBLIC_*` service env into the binary and shipped the tree-sitter wasm. Neuron's packaging is P4. |
| Codebuff env and URLs | Codebuff's web env schema (`env.ts`, `env-schema.ts`, `NEXT_PUBLIC_*`), the `WEBSITE_URL`/`LOGIN_WEBSITE_URL` constants and every link built from them. Test fixtures now use `example.*` hosts. | No Codebuff URL is left in source or tests. |

Removed tests went with the modules they covered (147 files). Other tests were
rewritten for what remains:

- `use-send-message` now drives a fake engine client.
- `cli-args` tests the real `parseArgs`.
- `exit-cleanly` and `settings` cover their new shapes.
- The others dropped cases for removed commands, props and modes.
- The e2e, tmux-POC and re-render perf tests needed Codebuff's built SDK and its
  `scripts/tmux`, so they were removed.

## What replaced them

| File | Role |
|---|---|
| `src/engine/protocol.ts` | TypeScript mirror of `rust/crates/neuron-tui/src/protocol.rs`: `UiEvent` and `EngineCommand` (serde `type`), `ClientMessage`, `Attachment` and `PermissionDecision` (serde `kind`), `PermissionReply`, `PROTOCOL_VERSION`. Fields that are `Option` in Rust are `T \| null`, since serde writes them as `null`. Also the JSON-lines codec: `encodeClientMessage`, `parseUiEventLine` (unknown event types are reported, not thrown) and `JsonLineSplitter`. Its tests use the JSON from the Rust serde tests. |
| `src/engine/neuron-engine-client.ts` | The `NeuronEngineClient` interface P2 implements (`start`, `submit`, `slash`, `setModel`, `listModels`, `interrupt`, `replyToPermission`, `subscribe`, `onExit`, `shutdown`), `engineArgs()` for `neuron engine --stdio` and its flags, and `runTurn()`, which submits and resolves at `turn_finished`. The P1 client is never connected: commands throw `EngineNotConnectedError`. |
| `src/hooks/use-send-message.ts` | Rewritten around `runTurn()`. It keeps the UI half: user bubble, AI message shell, timer, interrupts (they call `engine.interrupt()`), queue state and transcript checkpoints. |
| `src/types/agent-run.ts` | Local versions of the SDK types the UI read: `RunState` (what `--continue` saves; P2 adds the engine session id), `MessageContent`, `ToolName`. |
| `src/utils/user-shell.ts` | `!cmd` runner stub. It fails with a message shown in the bash bubble. |
| `src/utils/runtime-env.ts` | `IS_DEV`, `IS_TEST` and `IS_CI`, which came from Codebuff's env schema. |
| `src/utils/tool-display.ts`, `src/utils/logo.ts` | Display helpers kept from removed modules (`codebuff-client.ts`, `login/`). |
| `src/utils/exit-cleanly.ts` | Exit now runs local cleanup, then a bounded engine `shutdown`. |
| `src/utils/config-dir.ts` | Settings, chat history and logs now go to `~/.neuron/tui`. Freebuff used `~/.config/manicode`, which Codebuff itself still writes. `FREEBUFF_CONFIG_DIR` still overrides it. |
| `scripts/run-tests.ts` | `bun run test` without `find`, so it works on Windows. |
| `test/setup-env.ts` | Test preload: runtime flags only, no Codebuff service env. |

`IS_FREEBUFF` (`src/utils/constants.ts`) is pinned to `false`, so no env var can
turn back on the paths it gated. Its remaining 22 uses only choose
Freebuff/Codebuff wording, which the rebrand removes.

## Behaviour today (P1)

- `bun run dev` starts the UI to the empty chat screen (project picker at the
  home directory, git-root banner in a subfolder).
- Sending a message shows the user bubble, then "The Neuron engine is not
  connected yet: this UI build has no engine bridge." as the turn's error.
- `!cmd` and `/bash` show "Shell commands (!) are not available in this build of
  the Neuron UI yet."
- These slash commands remain: `help`, `new`, `copy`, `export`, `bash`, `image`,
  `history`, `queue`, `todo`, `interview`, `plan`, `review`, `theme:toggle`,
  `exit`, `mode:*` and `skill:*`.
- Typecheck is clean, and `bun run test` gives 2,190 passing tests, 0 failures
  and 24 skipped.
- Of the skips, 7 POSIX-only cases skip on Windows: direnv export, directory
  fsync, chmod, and a concurrent atomic rename. The rest are upstream skips,
  mostly tmux. Freebuff's CI ran on Linux, so these never ran on Windows
  before.

## Left for later

- **P2 (engine bridge):**
  - Implement `NeuronEngineClient`: spawn `neuron engine --stdio`, write
    `ClientMessage` lines, read `UiEvent` lines (stderr carries logs).
  - In `use-send-message.ts`, map events onto the existing handler state from
    `createEventHandlerState`, in the `runTurn` `onEvent` callback.
  - Map Neuron tool names to Freebuff renderers, and build the permission
    dialog.
  - Send attachments as `Submit.attachments`. Today they are inlined into the
    prompt as Freebuff did.
  - Record the engine session id in `RunState` so `--continue` resumes it.
  - Decide whether `!cmd` runs in the UI or in the engine.
- **P3:**
  - Sign-in screen, model picker, footer (git, ↑/↓ tokens, context %).
  - `utils/format-token-count.ts` is kept for the footer.
  - `@tanstack/react-query` is kept and still mounted in `index.tsx`, for the
    model-list polling.
- **P4 (rebrand and packaging):**
  - Remove the Codebuff/Freebuff names users can see: the logo in
    `utils/logo.ts`, "Codebuff will run commands on your behalf", the program
    name in `--help`, `IS_FREEBUFF` wording, and export file names.
  - Rename the internal ones: the `@codebuff/common` alias, `CODEBUFF_*`/
    `FREEBUFF_*` env vars, the `codebuff:send-followup` event, the
    `.freebuff-native-bundle.json` receipt, and the `codebuff`/`freebuff` names
    in direnv's steering list.
  - `bun build --compile` per release target. Freebuff's
    `cli/scripts/build-binary.ts` shows how it embedded the OpenTUI native
    library.
  - Bring back a reset-sequence sync test for Neuron's own launcher (the old
    one checked `release-core/launcher.js`).
- **Known issues:**
  - `writeFileAtomicAsync` can fail with `EPERM` on Windows when two renames
    target the same file at once. Checkpoint saves coalesce, so it is unlikely
    but possible.
  - In my launch check the UI drew the chat screen, but I could not type into
    the terminal: keyboard input and Ctrl-C exit in a real Windows terminal are
    covered only by unit tests so far.
