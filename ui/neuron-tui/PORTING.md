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
| Free-mode waiting room and session queue | The Freebuff session: store, API, display, identity, relaunch, instance owner, device key, chat admission and capacity wait. The model catalog, store, directory and the 1,900-line model selector. Streak, referral, premium reset and Freebucks. The Freebuff chat header, footer, controls, session summary and paused-plan notice. `/model`, `/reasoning`, `/end-session`, `/dashboard`. The Freebuff picks in settings. The `IS_FREEBUFF` build flag (removed in the rebrand, see below). | Freebuff's hosted free-capacity service. Neuron's model picker comes from the engine in P3. The message queue for prompts sent during a turn is kept: it is UI, not the waiting room. |
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
| `src/engine/neuron-engine-client.ts` | The `NeuronEngineClient` interface (`start`, `submit`, `slash`, `setModel`, `listModels`, `interrupt`, `replyToPermission`, `subscribe`, `onExit`, `shutdown`) and `engineArgs()` for `neuron engine --stdio` and its flags. It also has `runExchange()`/`runTurn()`. They run one exchange at a time per engine, so each `turn_finished` ends the right one, and resolve with the turn's `error`. |
| `src/engine/stdio-engine-client.ts` | P2: `StdioEngineClient` spawns `neuron engine --stdio`. It writes `ClientMessage` lines, reads `UiEvent` lines and sends stderr to the log file. `start()` resolves at `ready`. It rejects with the engine's `error` event, or its last `error:` stderr line (some launch checks print only that), or the exit code. Unknown event types are logged, not fatal. `resolveNeuronBinary()` tries `NEURON_BIN`, then a `neuron` beside the UI binary, then PATH. |
| `src/engine/engine-session.ts` | P2: starts the engine in the project folder at launch, or after the project picker. It restarts it on a project switch, and resumes the saved engine session with `--continue`. It routes events that belong to no turn into the stores: `ready`, `status`, `usage`, `models`, permission requests, notices (an `AskUserQuestion` notice waits for the next submit), and errors outside a turn. A prompt sent while the engine boots waits for the start (`whenEngineStarted`). |
| `src/engine/turn-events.ts` | P2: feeds a turn's events into Freebuff's handlers (`createStreamChunkHandler`/`createEventHandler`). `text_delta` becomes text; `reasoning_delta` becomes a reasoning chunk. Tool calls and results become `tool_call`/`tool_result` blocks. A result is matched to its call by `id`. Engines before the sign-in work sent no `id`; for those, results are matched first-in-first-out by tool name. A call left without a result (those engines sent none for a denied call) is closed at `turn_finished`. |
| `src/engine/tool-mapping.ts` | P2: Neuron tool names and inputs mapped onto Freebuff's renderers: `bash`/`PowerShell`→`run_terminal_command`, `read_file`→`read_files`, `write_file`, `edit_file`→`str_replace`, `glob_search`→`glob`, `grep_search`→`code_search`, `WebFetch`→`read_url`, `WebSearch`→`web_search`, `TodoWrite`→`write_todos`, `Skill`→`skill`. The engine's JSON tool outputs become what those renderers read. Windows `\\?\` path prefixes are stripped for display. Other tools keep their name and input. |
| `src/state/engine-store.ts` | P2: connection state, engine version, last `status` (model, mode, session, git, context %, Zero-X login), `usage`, `models`, the permission queue and a pending question. P3 adds the device code of a `/login` in progress, and `needsSignIn()`. The engine's signed-out stage is a `status` with `signed_in: false` and no session id; an API-key provider also has `signed_in: false`, but it has a session. Engines that send `status.sign_in_required` decide it directly. |
| `src/components/sign-in-panel.tsx` | P3: above the input. In the signed-out stage it says to type `/login`. During a `/login` it shows the link, the code and a countdown, and Esc cancels (an engine interrupt). After approval (the `session_starting` event, or the success notice from engines without it) it says "Starting your session…" until the engine's session is up. The engine opens the browser and keeps the token; the UI only sees the code. |
| `src/components/model-picker.tsx` | P3: `/model` lists the engine's catalog (`list_models`, refreshed every 30 s while open; only the Zero-X gateway has one). Unavailable models are greyed with the reason and can't be picked. `/model <name>` switches without it. |
| `src/components/engine-footer.tsx` | P3: one line under the input: model, permission mode (and plan mode), an untrusted-folder warning, git branch and counts, ↑/↓ tokens for the session, context %, and the Zero-X plan or an expired login. Parts that don't fit are dropped from the end. |
| `src/engine/attachments.ts` | P3: images go to the engine as `Submit.attachments`. Before, they were dropped and only "See attached image(s)" was sent. Files, folders and pasted text stay inlined into the prompt as Freebuff did, because the engine would only list their paths. |
| `src/components/permission-prompt.tsx` | P2: the permission dialog. `y` allows once, `a` allows for the session (only when the engine offers prefixes), `n` denies, `e` denies with a note. Keys are ignored for 300 ms so a keystroke meant for the input can't answer it. The chat input is disabled while a request waits. |
| `src/hooks/use-send-message.ts` | Rewritten around `runTurn()`. It keeps the UI half: user bubble, AI message shell, timer, interrupts (they call `engine.interrupt()`), queue state and transcript checkpoints. It saves the engine session id in `RunState`. |
| `src/commands/router.ts` | P2: a slash command the UI doesn't know goes to the engine when it is running (its own commands, such as `/login`, `/status`, `/cost` and `/context`). One typed while the engine boots waits for it. Its notices and errors show as system lines. `/new` also clears the engine session. The slash menu lists the engine's commands with `engineCommand: true`. |
| `src/cli-args.ts` | P3: `--model`, `--provider`, `--permission-mode`, `--allow-broad-cwd` and `--dangerously-skip-permissions` pass through to `neuron engine --stdio`. |
| `src/types/agent-run.ts` | Local versions of the SDK types the UI read: `RunState` (what `--continue` saves, with the engine session id), `MessageContent`, `ToolName`. |
| `src/utils/user-shell.ts` | `!cmd` runner stub. It fails with a message shown in the bash bubble. |
| `src/utils/runtime-env.ts` | `IS_DEV`, `IS_TEST` and `IS_CI`, which came from Codebuff's env schema. |
| `src/utils/tool-display.ts`, `src/utils/logo.ts` | Display helpers kept from removed modules (`codebuff-client.ts`, `login/`). |
| `src/utils/exit-cleanly.ts` | Exit now runs local cleanup, then a bounded engine `shutdown`. |
| `src/utils/config-dir.ts` | Settings, chat history and logs now go to `~/.neuron/tui`. Freebuff used `~/.config/manicode`, which Codebuff itself still writes. `NEURON_TUI_CONFIG_DIR` still overrides it. |
| `scripts/run-tests.ts` | `bun run test` without `find`, so it works on Windows. |
| `scripts/dev-against-mock.ts` | P2: `bun run dev:mock [prompt]` runs the UI against the real engine and the mock Anthropic service (`PARITY_SCENARIO:<name>` prompts), in a throwaway home and workspace. No account or network is needed. With `--signed-out` (P3), the engine starts signed out against `scripts/fake-zero-x-gateway.ts`, which approves a `/login` at the first poll and serves a two-model catalog. Logins go to a file in the temp home, never the OS keychain. |
| `test/setup-env.ts` | Test preload: runtime flags only, no Codebuff service env. |

## Rebrand (P4)

- `IS_FREEBUFF` is gone. Each of its uses kept the branch Neuron already ran
  (`false`), so behaviour did not change.
- Users now see Neuron's names:
  - a new NEURON block logo in `utils/logo.ts`;
  - "Neuron will run commands on your behalf", and "You started Neuron in a
    subdirectory…";
  - `neuron` in `--help`, the terminal title, the exit hint
    (`neuron --continue <id>`, from `CLI_COMMAND`) and `neuron-chat-<id>.md`
    exports;
  - the renderer-library error messages.
- The help banner had kept Codebuff's billing line ("/subscribe · /usage · 1
  credit = 1 cent") and hid `/model`. Now it lists `/model` and `/login` in
  place of `@agents` (Freebuff's agent store).
- Env vars are renamed to `NEURON_TUI_*`: `CONFIG_DIR`, `VERSION`, `IS_BINARY`,
  `TARGET`, `RG_PATH`, `SCROLL_MULTIPLIER`, `PERF_TEST`, `LAUNCHER_PID`,
  `NO_TERMINAL_WATCHDOG`, `RUN_TMUX_TESTS` and `DEBUG`. `NEURON_TUI_EDITOR`
  replaces `CODEBUFF_CLI_EDITOR`, and `NEURON_EDITOR` replaces `CODEBUFF_EDITOR`.
  The agent-store and SDK knobs (`CODEBUFF_TRUSTED_AGENT_PUBLISHERS`,
  `CODEBUFF_API_KEY`, `CODEBUFF_WASM_DIR`, ...) are gone.
- Internal names are renamed too:
  - the alias `@codebuff/common` → `@neuron-tui/common`;
  - `codebuff:send-followup` → `neuron:send-followup`;
  - the watchdog's `neuron-tui-watchdog-disarm-*` files and
    `__NEURON_TUI_OWNER_START__` token;
  - the `.neuron-tui-native-bundle.json` receipt;
  - `neuron-tui-clipboard-images`;
  - `CodebuffFileSystem` → `ProjectFileSystem`.
- The `@` file picker reads `.gitignore` only, as the engine does. It no longer
  reads `.codebuffignore` or `.manicodeignore`.
- What stays: the "Derived from Codebuff/Freebuff" headers and the LICENSE and
  NOTICE files (Apache-2.0 attribution), comments about where code came from,
  and `codebuff/<agent>@<version>` ids in tests of the id parser.

### A repository's `.envrc` can no longer steer the engine

Freebuff imports a project's direnv environment at startup (`init/init-direnv.ts`)
and drops the variables that would reconfigure the CLI itself. P2 made the UI
start the engine with its environment. So the drop list was Codebuff's
(`CODEBUFF_*`, `FREEBUFF_*`, `NEXT_PUBLIC_*`), and a cloned repo could have
done any of these:

- set `NEURON_BIN` to run its own program as the engine;
- set `NEURON_API_BASE` to send the Zero-X session to its own host;
- set a provider key or base URL to route prompts elsewhere.

The list now drops `NEURON_*` (the engine's and this UI's knobs) and the
providers' prefixes (`ANTHROPIC_`, `OPENAI_`, `AZURE_OPENAI_`, `XAI_`,
`GROK_`, `DASHSCOPE_`, `OPENROUTER_`, `MISTRAL_`, `CLAWD_`). It still imports
`PATH`, so the agent's commands see the project's tools. The `neuron` binary,
though, is looked up on the PATH from before the import
(`engine/startup-env.ts`), so a repo can't put its own `neuron` first.
`HTTPS_PROXY` stays importable, as upstream chose; TLS trust variables stay
blocked.

## Packaging (P4)

- `bun run build:binary <version>` (`scripts/build-binary.ts`, from Freebuff's)
  compiles the UI into `bin/neuron-tui[.exe]`. It embeds this platform's OpenTUI
  native library and bakes in `NEURON_TUI_VERSION`/`NEURON_TUI_TARGET`. A
  project's `.env` and `bunfig.toml` are never read. `OVERRIDE_TARGET`,
  `OVERRIDE_PLATFORM` and `OVERRIDE_ARCH` cross-compile (`release.yml` builds
  macOS x64 on Apple silicon). On Windows the binary is about 114 MB, most of
  it the Bun runtime.
- `release.yml` builds `neuron-tui-<target>` in each target's build job, beside
  `neuron-<target>`. It writes the `.sha256`, smoke-tests `--version`, checks the
  Linux glibc floor, attests provenance, lists the asset in `SHA256SUMS`, and
  checks `neuron-tui --version` after each install.
  `THIRD_PARTY_NOTICES.md` gains a "Full-screen UI" section: this package's
  LICENSE and NOTICE, and the licenses of the 190 npm packages compiled into it.
- `neuron` (Rust, `full_screen.rs`) starts the UI in place of its line REPL
  when stdin and stdout are terminals and `neuron-tui` sits beside it (or at
  `NEURON_TUI_BIN`). It passes its REPL flags on (`--model`, `--provider`,
  `--permission-mode`, `--allowedTools`, `--reasoning-effort`,
  `--allow-broad-cwd`) and sets `NEURON_BIN` to itself, so the UI runs it as the
  engine. `--no-tui`, `NEURON_NO_TUI=1` or `--base-commit` keep the REPL. On Unix
  `neuron` execs the UI. On Windows it waits for it, with the console's Ctrl-C and
  Ctrl-Break marked handled, so the parent is not killed under the UI.
- The installers put `neuron-tui` beside `neuron`. The npm and pip launchers
  keep it verified and cached like `neuron`: postinstall fetches it, and a
  terminal launch fetches it on demand. They pass its path as `NEURON_TUI_BIN`.
  A failure about the UI is a warning, never a failed install or launch: `neuron`
  then runs its REPL. `NEURON_NO_TUI=1` skips the download.
- `bun run dev:mock --compiled` runs the compiled UI against the mock, and
  `--via-neuron` runs a bare `neuron` to check the hand-off. Both were checked by
  hand on Windows: the binary loads its renderer and runs a turn, and a bare
  `neuron` opens it with itself as the engine.
- CI (`rust-ci.yml`, job `neuron-tui`) typechecks, tests and compiles the UI on
  Ubuntu and Windows. `ui/react-tui` (the Ink prototype) and the root npm
  workspace that held it are gone.

## Behaviour today (P4)

- `bun run dev` starts the UI and, once a project folder is known, the engine
  in it. The status bar says "starting the engine..." until `ready`.
- A prompt runs a real engine turn. Streamed text, reasoning, tool calls and
  their results render with Freebuff's blocks. Permission requests open the
  permission dialog. The footer shows the engine's status and usage.
- Signed out of Zero-X, the engine still starts. The sign-in panel asks for
  `/login`, which shows the device code and link until the sign-in is approved.
  `/logout` signs out. `/model` opens the model picker.
- If the engine cannot start, its reason shows in the chat and the status bar.
  Examples: no `neuron` binary, or a too-broad folder. A send then ends with
  "The Neuron engine is not running: <reason>".
- Checked by hand with `bun run dev:mock` on Windows:
  - `streaming_text` renders the streamed reply.
  - `bash_permission_prompt_approved` opens the permission dialog, with `tee`
    offered for the session.
  - `--signed-out /login` shows the code `BCDF-GHJK` and its link. After the
    fake approval, the footer reads `auto · workspace-write · untrusted
    folder · ctx 0% · Zero-X pro`.
  - Opening the session after a sign-in took about 40 s on the debug build,
    most of it starting MCP servers. They now start in the background (see
    below), so `ready` and the first turn no longer wait for them.
  - After the rebrand, `streaming_text` shows the NEURON logo, "Neuron will
    run commands on your behalf", and the footer with live token counts.
- `bun run check:console` (Windows) runs the compiled UI in a real
  pseudoconsole (ConPTY, which Windows Terminal runs programs in) and uses it
  through key presses and resizes. The scripts are in
  `scripts/windows-console/`, played by `ConPtyDrive.cs`, which is compiled with
  Windows' own `csc.exe`. The checks cover:
  - typing a prompt and Enter, the streamed reply, and a permission request
    answered with `y` (and the command run);
  - resizing to 60×28 (the small logo, wrapped text, a shorter placeholder) and
    to 150×42, then a request denied with `n`;
  - a bare `neuron` handing the console to the UI (`NEURON_TUI_BIN`), `a` (allow
    for the session), and Ctrl-C twice ("Press Ctrl-C again to exit", then the
    exit), with `neuron` returning the UI's exit code;
  - `neuron --continue <chat>` reopening a saved chat (its transcript, and the
    engine resuming its session).

  All 5 pass on Windows 11. Each scenario waits for a turn to end before Ctrl-C:
  during a turn Ctrl-C interrupts it instead. Bugs it found are fixed:
  - The exit hint was never shown on Windows. The `process.on('exit')` listener
    ran after the console was gone (EPIPE), and a write from the UI races its
    renderer's teardown. When `neuron` starts the UI, the UI now leaves the hint
    in a file (`NEURON_TUI_EXIT_MESSAGE_FILE`) and `neuron` prints it once the UI
    has exited. Run directly, the UI prints it itself after the renderer has
    restored the main screen. No hint for a chat nothing was asked in.
  - `neuron --continue` was an unknown option. `neuron` now passes
    `--continue [chat]` to the UI, and says so plainly where the UI can't run it.
    A bare `--continue` skips chats that saved no messages.
  - The driver's screen model is fed in a `try`, so the pseudoconsole's output
    is always read.

  The hint itself can't be checked there. Under the pseudoconsole that
  kernel32's `CreatePseudoConsole` provides, the console ends for every process
  in it as OpenTUI's native renderer tears down (writes fail with error 233,
  about 6 ms after the teardown starts), so nothing printed after that arrives.
  A classic console window and Windows Terminal 1.24 keep the console. In both,
  a bare `neuron --continue` (with a temporary auto-exit) reopened the chat, and
  `neuron`'s print of the hint succeeded. That was checked by hand, not by
  `check:console`.
- Changed after the end-to-end run against a local gateway:
  - A failed tool (a refused write, a command that exits non-zero) shows as
    failed or refused, in the error colour, instead of like a success.
  - Single newlines in a reply stay line breaks, and a new text block starts a
    new paragraph. Backslashes in Windows paths are kept. Tool output is shown
    as plain text, not parsed as Markdown.
  - Chats are kept per project path (`projects/<folder>-<hash>`), so two repos
    with the same folder name no longer share `/chats` and `--continue`. Chats
    saved before under the folder name alone are still read.
  - The PowerShell watchdog runs in the temp folder, so its module cache never
    lands in the project.
  - The model picker greys out `blurred` models until `available_again_at`, marks
    `degraded` and `busy` ones, and says "not configured on this gateway".
  - After an interrupt, the engine's REPL wording ("press Ctrl-C again to quit")
    is not shown; the sign-in panel's `/login` is the one sign-in instruction.
- `!cmd` and `/bash` show "Shell commands (!) are not available in this build of
  the Neuron UI yet."
- These slash commands are the UI's own: `help`, `new`, `copy`, `export`,
  `bash`, `image`, `history`, `queue`, `todo`, `interview`, `plan`, `review`,
  `theme:toggle`, `exit`, `mode:*` and `skill:*`. Any other goes to the engine.
- Typecheck is clean, and `bun run test` gives 2,238 passing tests, 0 failures
  and 24 skipped.
- `src/engine/__tests__/engine.integration.test.ts` passes 5 tests against the
  real engine: 4 turns against the mock service, and the signed-out start,
  `/login`, catalog and `/logout` against the fake gateway. It is not part of
  `bun run test`, because it needs the Rust binaries and takes about 3 minutes
  on a debug build.
- Of the skips, 7 POSIX-only cases skip on Windows: direnv export, directory
  fsync, chmod, and a concurrent atomic rename. The rest are upstream skips,
  mostly tmux. Freebuff's CI ran on Linux, so these never ran on Windows
  before.

## Left for later

- **Still open from P3:**
  - Decide whether `!cmd` runs in the UI or in the engine.
  - `@tanstack/react-query` is still mounted in `index.tsx`, but nothing uses
    it now: the model list polls with a timer. Removing it changes the
    lockfile, so it waits for a dependency pass.
  - The agent modes (DEFAULT, LITE, MAX, PLAN; the `< DEFAULT` toggle, the mode
    divider, `--lite`/`--max`/`--plan`) change only how the UI presents a turn.
    The engine ignores them. Hide them, or map PLAN to the engine's plan mode.
- **After P4:**
  - Bring back a reset-sequence sync test for Neuron's own launcher (the old
    one checked `release-core/launcher.js`).
  - The release build of the UI runs only in `release.yml`. A tagged release
    is its first full test on Linux and macOS: the Linux glibc floor for the
    Bun binary, the macOS x64 cross-compile, and the install checks.
- **Known issues:**
  - `writeFileAtomicAsync` can fail with `EPERM` on Windows when two renames
    target the same file at once. Checkpoint saves coalesce, so it is unlikely
    but possible.
  - A key typed in the first moment after the UI draws, while OpenTUI still
    reads the terminal's answers to its capability queries, can be dropped. The
    console check saw "PARITY…" arrive as "ARITY…" once, so its scripts wait
    for the engine's footer before typing.
  - After the UI exits, the engine can take a few seconds to finish saving and
    stopping (MCP servers still starting are stopped within 3 s). The UI waits
    for it for one second.
  - The console check covers the inbox pseudoconsole, not the Windows Terminal
    app's own rendering (fonts, colours, its key encoding before ConPTY). See
    above for why it can't see the exit hint.
  - A launch refusal (a too-broad folder) arrives as the protocol's `error`
    event on stdout, and the client reads stdout to its end before it handles
    the exit. An older engine build seemed to print it only on stderr. The client
    still falls back to the last `error:` stderr line, in case an event never
    comes.
  - In a folder under a git repo, Freebuff's banner offers to switch to the git
    root, even when that root is the home folder, which the engine then refuses
    as too broad.
  - The engine gets the UI's environment after the direnv import, minus the
    blocked variables. Its own lookups on PATH (`node`/`npx` for MCP servers)
    can therefore find a project's tools first, as they would in a shell that
    loaded direnv.
