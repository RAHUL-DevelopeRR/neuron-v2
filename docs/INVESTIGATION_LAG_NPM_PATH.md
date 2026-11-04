# Investigation: terminal lag, npm search, and `neuron` missing from PATH (2026-11-04)

Branch `fix/lag-npm-path`, cut from `feat/harness-gaps` (7b9db41); worktree `neuron-lagpath-wt`.
Local commits only: nothing was pushed, published or tagged.

## 0. A finding that came first: Defender quarantines `neuron.exe`

Running the npm-installed `neuron` (7.0.0-alpha.4) on this Windows 11 machine crashed the launcher:

```
Error: spawn UNKNOWN   (errno -4094)   at main (.../@zero-x.live/neuron/bin/neuron.js:84:17)
```

Microsoft Defender's history (`Get-MpThreatDetection`) has two detections at 18:22 and 18:24 today:

- File: `%APPDATA%\npm\node_modules\@zero-x.live\neuron\bin\native\neuron.exe`
- Threat: `Trojan:Win32/Bearfoos.A!ml`, from Defender's machine-learning heuristic
- Action: quarantined (`ActionSuccess=True`)
- Result: `bin\native\` still has `neuron.exe.sha256` but no `neuron.exe`. The orphaned `neuron-tui.exe` from that run
  was still alive.

**This is very likely a large part of "Neuron does not work on new devices".** A `!ml` verdict on an unsigned,
low-reputation binary is the classic false positive, and it hits every new Windows machine until the binary has
reputation. Without the fix below, the launcher crashed with an unhandled exception and the user had no hint why.

**Fixed here:** `npm/neuroncli/lib/start-error.js` and `bin/neuron.js:87`.

- The synchronous throw from `spawn()` is now caught.
- The `error` event and the throw both print one actionable line:
  - for a blocked binary: Windows would not start the binary; check Windows Security > Protection history; reinstall;
  - for a missing binary: the file is missing and an antivirus may have removed it.
- Tests: `test/start-error.test.js`.

**Recommended (needs Rahul):**

1. Sign the Windows binaries, `neuron.exe` and `neuron-tui.exe`, with Authenticode. Azure Trusted Signing is the
   cheapest route for an individual or small org. Sign in `release.yml` before the checksums are computed.
2. Submit the alpha.4 `neuron.exe` to Microsoft as a false positive (https://www.microsoft.com/wdsi/filesubmission).
3. Add a Windows smoke step to release CI that runs the downloaded asset with Defender real-time protection on.

## 1. Terminal lag

### 1.1 What was measured, and how

`ui/neuron-tui/scripts/bench-typing.tsx` (new, committed) is an in-process benchmark on OpenTUI's test renderer. It
lays out what `chat.tsx` draws: the `ChatHeader` with its logo, the transcript through `MessageWithAgents`, and
`MultilineInput`. It measures:

- idle CPU and frame requests over 3 s, with the logo sheen on and off;
- keystroke cost, from the input's state change to the drawn frame, averaged over 55 keystrokes:
  - with an empty chat;
  - with 15 messages (the most `chat.tsx` shows before "load previous", `use-chat-messages.ts:20`) of 20-line
    answers;
  - with 15 messages of 200-line answers.

Run it with `NODE_ENV=production bun run scripts/bench-typing.tsx`.

The streaming path already has its own benchmark on `fix/tui-provider-and-lag`, from another session: `bun run
bench:streaming` (commits afdc2eb and f97c869). I did not duplicate it, and I deliberately left the files it changes
alone.

The machine was loaded by other sessions' builds, so numbers vary between runs. Two runs, React production build:

| Scenario | Run 1 | Run 2 |
|---|---|---|
| Idle, logo sheen on | 146 ms CPU/s | 115 ms CPU/s |
| Idle, logo sheen off | 57 ms CPU/s | 47 ms CPU/s |
| Keystroke, empty chat (mean / p95) | 22.7 / 88 ms | 3.9 / 13.6 ms |
| Keystroke, 15 short answers | 12.9 / 68 ms | 11.1 / 29.9 ms |
| Keystroke, 15 long answers | 22.4 / 84 ms | 9.4 / 36.6 ms |
| One drawn frame (layout and draw) | 0.7-2.7 ms | 0.7-2.1 ms |

A real `neuron-tui.exe` process sitting idle measured 31 ms CPU in 10 s (0.3% of a core) with `Get-Process`
sampling. I don't know which screen it was showing.

### 1.2 Root causes, ranked

1. **The logo sheen ran forever while idle (fixed).**
   - `chat.tsx:1322` turns it on whenever the header is in view and the input has focus, which is always true on a
     new or short chat.
   - `use-sheen-animation.tsx` ticks every 150 ms (`logo.ts:94`). Each tick re-renders the logo as one `<span>` per
     character, and OpenTUI then lays out and draws a frame.
   - Cost: about 60-90 ms of CPU per second (6-9% of a core) while you do nothing.
   - The ticks are about 7 frames a second competing with keystrokes on Bun's single JS thread, which matters most on
     slow machines and in Windows conhost.
   - **Fix:** the sheen runs `SHEEN_MAX_PHASES = 2` phases (one fill and one unfill, about 15 s), then rests. It sweeps
     again when it is enabled again, for example when the header scrolls back into view. Test:
     `src/hooks/__tests__/use-sheen-animation.test.tsx`.

2. **Streaming text advances in 100 ms steps, and every token re-rendered too much (being fixed on
   `fix/tui-provider-and-lag`).**
   - `message-updater.ts:29` sets `DEFAULT_FLUSH_INTERVAL_MS = 100`.
   - That branch measured, and fixed:
     - the status line re-rendering on every token;
     - every finished message re-rendering on each transcript update (a new message tree each time);
     - immer drafts of the whole transcript on every update;
     - full Markdown re-parsing of the streaming block.
   - With those fixed it drops the flush to 33 ms.
   - **Recommendation:** merge that branch rather than redo it.

3. **Keystrokes re-render the whole `Chat` component (not fixed, measured only in part).**
   - `inputValue` lives in `Chat` (`chat.tsx:140`), so each keystroke re-runs the 1,500-line component: dozens of
     hooks, `useMemo` dependency checks, the keyboard-handler object (`chat.tsx:896`), the status bars and the input
     bar.
   - The transcript itself is safe: `MessageWithAgents` is `memo`'d (`message-with-agents.tsx:105`) and its props stay
     the same while typing. The 200-line case therefore costs no more than the 20-line case.
   - The harness does not mount `Chat` itself, so its 4-23 ms per keystroke is a lower bound.
   - **Recommendation:** move the input state into a small `ChatInputContainer`, or into the zustand store with
     selector subscriptions, so a keystroke re-renders only the input, the slash/mention menus and the counters.
     Then add `Chat` to the benchmark to measure it. This is a medium-sized refactor of `chat.tsx`, which other lanes
     also edit, so I did not do it here.

4. **Several timers run during a turn.**
   - Spinner every 100 ms and shimmer every 160 ms (`live-activity.tsx:25-26`).
   - Elapsed clocks every 1 s (`status-bar.tsx:109`, `use-elapsed-time.ts:105`).
   - Cursor blink every 500 ms (`input-cursor.tsx:27`).
   - Each is a small React commit plus a frame. OpenTUI renders on demand and merges requests up to `maxFps` 60, so
     these are cheap; they add up only with cause 3.
   - **Recommendation:** drive spinner and shimmer from one shared 100 ms clock, and stop the blink while a turn
     streams.

5. **Engine stdio is not a bottleneck.**
   - Each event is one flushed JSON line (`engine_stdio.rs:265`); text deltas go out as they arrive.
   - `tool_input_progress` is limited to one every 100 ms per call and `output_progress` to one every 250 ms
     (`turn_sink.rs:36,38`).
   - At model token rates (30-150 deltas a second) that is well under 1 ms of work per event.
   - The TUI's own batching (cause 2) decides what you see. I did not measure the engine's event rate against the
     mock engine, because a debug engine build for the mock costs about 4 GB and the disk is tight.

6. **The line REPL holds back fenced code blocks.**
   - `MarkdownStreamState::push` renders at the last safe boundary (`render.rs:992`). Inside a fenced code block
     there is none until the fence closes (`render.rs:1010`), and lists wait for the next unindented line.
   - A 300-line code block therefore shows nothing for as long as the model takes to write it, then appears all at
     once, which reads as a freeze.
   - **Recommendation:** inside a fence, emit each complete line as it arrives, highlighted with the fence's
     language. The highlighter is line-oriented, so the fence state can carry across chunks. Keep the current
     behaviour for lists and tables. It is a contained change in `render.rs` with good existing tests
     (`render.rs:1158+`), but it changes REPL output, so it is left for its own PR.

7. **Windows specifics.** The launch chain is the npm launcher (Node), then `neuron.exe`, then `neuron-tui.exe` (Bun),
   which spawns the engine. Keys go straight to `neuron-tui` through the inherited console, so the chain adds no
   per-key latency. Its cost is start-up time and, as section 0 shows, three processes for antivirus to inspect.

## 2. `npm search neuron` does not find the package

Evidence, from the registry on 2026-11-04:

- `npm view @zero-x.live/neuron`:
  - versions 6.2.0, 7.0.0-alpha.3 and 7.0.0-alpha.4; `latest` is 7.0.0-alpha.4;
  - published by GitHub Actions trusted publishing, with SLSA provenance attestations.

  Published and indexed correctly: this is not search-index lag or a publish failure.
- The unscoped name `neuron` belongs to an unrelated 2022 job-queue package.
- `GET /-/v1/search?text=neuron&size=250`: 333 results, and `@zero-x.live/neuron` is **rank 53**.

  `npm search` shows **20** results by default (`searchlimit`), so it never appears.
- Searches where it does appear:

  | Query | Rank |
  |---|---|
  | `neuron coding agent` | #1 |
  | `@zero-x.live/neuron` | #1 |
  | `zero-x` | #2 |
  | `keywords:coding-agent neuron` | #2 |

  It is missing from the top 250 for `neuron agent` and `neuron ai`, and outside the top 50 for `neuron cli`.
- Ranking mixes text relevance with popularity. The package has 24 downloads a month against long-established
  `neuron-*` packages.

**Why:**

- The bare word collides with 300+ packages.
- The package is scoped, so its name contributes only `neuron` to matching.
- Its description and keywords did not contain the words people actually type: "ai agent", "cli", "neuron-cli",
  "llm".

**Changed here:** `npm/neuroncli/package.json`. Takes effect on the next publish.

- The description leads with "Neuron: an AI coding agent CLI for your terminal…".
- Keywords added: `neuron-agent`, `neuron-cli`, `neuroncli`, `ai-agent`, `code-assistant`, `ai-coding`, `llm`,
  `agentic`, `tui`, `developer-tools`, `zero-x.live`, alongside the existing ones.
- Package tests pass.

**Recommended:**

1. Do not rely on search: put the exact command, `npm i -g @zero-x.live/neuron`, on zero-x.live, the README and the
   release notes.
2. Optionally publish an unscoped alias that depends on the scoped package and exposes the same `bin`.
   - `neuron-agent` and `neuron-code` are free.
   - `neuroncli` is free too, but npm's similarity rule will likely refuse it because `neuron-cli` exists.
3. Publish prereleases under `next` as `release.yml` intends. alpha.4 is on `latest` and there is no `next` tag, so
   that release was probably a manual run with `npm_dist_tag=latest`.
4. Mirror the PyPI keywords (`pyproject.toml:16`).

## 3. `neuron` not on PATH after install on new devices

### 3.1 Install paths and their gaps (before this branch)

| Path | Where `neuron` goes | PATH handling (before) | Gap |
|---|---|---|---|
| `install.ps1` | `%LOCALAPPDATA%\Programs\Neuron\bin` | Only with `-AddToPath`. That switch cannot be passed through `irm \| iex`, which is the advertised command. | On a fresh machine the dir is never on PATH; the user saw a warning and a long script-block command. |
| `install.sh` | `~/.local/bin` | Never edits files; prints an `export` line | macOS has no `~/.local/bin` on PATH. Many Linux distros add it from `~/.profile` only if it existed at login, so a newly created dir needs a new login. |
| npm | `<prefix>` on Windows (`%APPDATA%\npm`), `<prefix>/bin` elsewhere | npm's job | npm prefix not on PATH (custom prefix, `sudo`-less setups, nvm/fnm version switches). Windows needs a new terminal after Node's installer adds `%APPDATA%\npm`. |
| PyPI | pip's `Scripts`/`bin` | pip warns only | `pip install --user` puts scripts in `%APPDATA%\Python\Python3xx\Scripts` or `~/.local/bin`, often not on PATH. There was no `python -m` fallback. |

### 3.2 What other installers do (public docs and scripts only)

| Tool | Default PATH handling | Windows method | Opt-out | Notes |
|---|---|---|---|---|
| Codex CLI | `install.sh` writes a marker block `# >>> Codex installer >>>`; macOS zsh `~/.zprofile`, Linux `~/.bashrc`/`~/.zshrc` | Prepends `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin` to user PATH and the current session | — | `codex doctor` lists every `codex` on PATH. Sources: https://releases.openai.com/codex/install.sh, `install.ps1`, openai/codex PR 22336. |
| uv | `env` script sourced from `.profile`, `.bashrc`, `.zshrc`, fish `conf.d` | Raw registry read, REG_EXPAND_SZ, broadcast | `UV_NO_MODIFY_PATH` | Skips if already on PATH; writes `$GITHUB_PATH` in CI. Source: https://docs.astral.sh/uv/reference/installer/ |
| rustup | `. "$HOME/.cargo/env"` in `.profile`, bash files, `.zshenv`, fish `conf.d` | `HKCU\Environment\PATH` (REG_EXPAND_SZ) plus `WM_SETTINGCHANGE` | `--no-modify-path` | |
| Bun | Edits `.zshrc`, `config.fish` or `.bashrc` | Registry plus `SendMessageTimeout` broadcast; updates `$env:PATH` | `-NoPathUpdate` | |
| Deno | Prompted edits with backups | Edits user PATH | — | Docs say to open a new terminal and check `npm config get prefix`. |
| opencode | First rc file found, `# opencode` marker | — | `--no-modify-path` | Appends to `$GITHUB_PATH`. |
| Goose | Asks; with no TTY, prints instructions | Prints instructions only | — | Its suggested command copies the merged PATH into the user PATH, an anti-pattern. |
| Gemini CLI, Aider | Rely on npm or uv/pipx | — | — | Docs blame PATH; Aider's documented fallback is `python -m aider`. |

The common design:

- install to `~/.local/bin` or `%LOCALAPPDATA%\Programs\<tool>\bin`;
- **edit PATH by default** with an idempotent marked block and an opt-out;
- on Windows, read the raw user `Path`, write REG_EXPAND_SZ and broadcast `WM_SETTINGCHANGE`;
- tell the user how to fix the current shell;
- detect a shadowing copy;
- provide a doctor check.

### 3.3 Changed here

- **`install.sh`** (`install.sh:396-490`): when the dir is not on PATH, it now adds a marked block to the startup
  files a new terminal reads:

  | Shell | File |
  |---|---|
  | zsh | `${ZDOTDIR:-~}/.zshrc` |
  | bash | `~/.bashrc`, plus `~/.bash_profile` on macOS or when that file exists (login shells skip `.bashrc`) |
  | fish | `~/.config/fish/conf.d/neuron.fish` (`fish_add_path`) |
  | other | `~/.profile` |

  - The block, `# >>> neuron >>>`, is added once, so reruns change nothing.
  - A file without a final newline is handled.
  - A dir containing `"`, `$`, `` ` `` or `\` is not written; the script prints manual instructions instead.
  - It prints the `export`/`fish_add_path` line for the current shell.
  - Opt out with `--no-modify-path` or `NEURON_NO_MODIFY_PATH=1`.
- **`install.ps1`** (`install.ps1:177`, `:349`): adding to the user PATH is now the default, through the existing
  `Add-UserPathEntry`, which does the raw registry read, the REG_EXPAND_SZ write and the broadcast.
  - Opt out with `-NoModifyPath` or `NEURON_NO_MODIFY_PATH=1`.
  - `-AddToPath` is still accepted and overrides the environment variable, so old commands work.
  - The fix applies to `irm … | iex` too.
- **`neuron doctor`** has a new `PATH` check (`doctor.rs:805`, after "install source").
  - It finds every `neuron` (and `.exe`/`.cmd`/`.bat`/`.ps1` on Windows) on PATH, in order.
  - It works out how this binary was installed, from where it lives:
    - npm: it is in `node_modules`, so it expects npm's shim dir (`<prefix>` on Windows, `<prefix>/bin` elsewhere)
      and mentions `npm config get prefix`;
    - npm or pip launcher cache: it is in `neuroncli/<version>`;
    - standalone.
  - It reports:
    - **ok** when the first `neuron` on PATH is this install;
    - **warn** when no `neuron` is on PATH, with the exact folder and per-OS command;
    - **warn** when another copy comes first.
  - JSON fields: `install_kind`, `commands_on_path`, `runs_this_install`.
- **PyPI:** `neuron_cli/__main__.py` makes `python -m neuron_cli` work as a fallback.

### 3.4 Recommended next

1. Publish these installer changes in the next release.
2. Make the install docs show the shorter `irm …/install.ps1 | iex`, which now adds to PATH.
3. In the npm `postinstall` (`scripts/install.js`), add a soft check: if `npm prefix -g`'s bin dir is not on PATH,
   print the folder and `neuron doctor`. Like pip, npm cannot fix PATH safely itself.
4. Optionally add `neuron setup-path` (as `uv tool update-shell` or `volta setup` do) that runs the same rc and
   registry logic for npm and pip installs.
5. In CI, also write `$GITHUB_PATH` when `GITHUB_ACTIONS` is set.

## 4. Verification

| Area | Command | Result |
|---|---|---|
| Rust doctor | `cargo test -p rusty-claude-cli --bin neuron doctor` | 10 passed, 5 of them new |
| Rust doctor | `--test output_format_contract` | 11 passed (now 7 checks) |
| Rust lint | `cargo fmt`, `cargo clippy -p rusty-claude-cli --all-targets -D warnings` | clean for this crate. The pre-existing `plugins` `duration_suboptimal_units` lint (Rust 1.95, unchanged code) still fails CI-style clippy; this run allowed only that lint. |
| npm | `node --test "test/*.test.js"` | 23 passed, 4 skipped (POSIX-only), 3 of them new |
| `install.ps1` | `pytest tests/test_install_scripts.py -k InstallPs1` | 20 passed, 1 skipped (CI-only registry test), 5 new subtests |
| `install.ps1` | real `HKCU\Environment\Path` compared before and after | unchanged |
| `install.sh` | `pytest -k InstallSh` | Cannot run on this Windows host: 32 tests already fail on HEAD under Git Bash, and there is no WSL distro. |
| `install.sh` | new functions run directly in a temp HOME | correct blocks for bash, zsh, fish and sh; reruns idempotent; dirs with spaces handled |
| `install.sh` | new tests (`test_adds_the_install_dir_to_the_shell_startup_file_once`, `test_startup_file_follows_the_shell`, `test_no_modify_path_leaves_startup_files_alone`) | run on Linux CI (`packaging-ci.yml`) |
| PyPI and packaging | `tests/test_packaged_launcher.py`, `test_npm_package.py`, `test_packaging_metadata.py` | pass |
| TUI | `bun test` on the sheen and header tests; `tsc --noEmit` | pass, clean |
| TUI | full `bun run test` | 2598 pass, 24 skip, 2 fail. The failures were `queue-panel` and `queue-editing` timing tests while the machine was loaded; both files pass on their own (24/24). |
