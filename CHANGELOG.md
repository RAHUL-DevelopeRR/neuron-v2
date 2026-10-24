# Changelog

Notable changes to Neuron. Versions follow [semantic versioning](https://semver.org/).
The release workflow copies a version's section into its GitHub release notes.

## 7.0.0-alpha.1 (2026-10-24)

The first Neuron alpha, and the first release built and published by this repository's
release pipeline. It is a GitHub prerelease and carries the work planned as 6.3.0,
which was never released. It adds a full-screen terminal UI, signs in to the Zero-X
gateway with a device code and keeps the login in the OS credential store, makes the
agent loop survive errors, cancellation and long sessions, gates every tool call by
what it actually does, keeps secrets and untrusted workspace settings away from the
model, and sends far smaller requests. Expect rough edges; the known ones are listed
first.

### Known limitations

- A prerelease never becomes "Latest", so `releases/latest` links, the install scripts
  run without a version and `npm install @zero-x.live/neuron` do not reach it. Install
  it by version:
  `curl -fsSL https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v7.0.0-alpha.1/install.sh | sh -s -- --version 7.0.0-alpha.1`,
  or in PowerShell
  `& ([scriptblock]::Create((irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/download/v7.0.0-alpha.1/install.ps1))) -Version 7.0.0-alpha.1`.
  Once the alpha is published to npm, it is `@zero-x.live/neuron@next` there.
- PyPI uploads are paused, so the alpha is not on PyPI; the Python launcher follows
  later.
- The binaries are not code-signed (Windows) or notarized (macOS).
- The full-screen UI's hands-on console checks ran on Windows; this release is its
  first build and install test on Linux and macOS. `neuron --no-tui` keeps the line
  REPL.
- In the full-screen UI, shell commands (`!cmd`, `/bash`) are not available yet, the
  DEFAULT/LITE/MAX/PLAN mode toggle changes only how a turn is shown, the footer's token
  and git counters update when a turn ends, and a key typed in the first moment after
  the UI appears can be lost.
- The full-screen UI cannot trust a folder yet. Run `neuron --no-tui` in the folder once
  and press 1 when it asks, or add the folder (or one above it) to `trustedRoots` in
  `~/.neuron/settings.json`, then restart Neuron. Until then the folder's own settings,
  hooks and MCP servers are ignored.
- In the full-screen UI, a `PATH` change from an allowed `.envrc` reaches the engine, so
  the engine's own tool lookups (such as `node` and `npx` for MCP servers) use the
  imported `PATH` and can find a project's tools first, as in a shell that loaded
  direnv. That includes commands Neuron runs without asking, such as `git status`: a
  folder the `.envrc` adds to `PATH` can run that repository's own programs without a
  prompt. Only allow an `.envrc` you have read.
- Hooks run with Neuron's own environment: they inherit every variable Neuron has,
  including a session token or key in `NEURON_TOKEN` and provider keys you exported.
  They also get the event as JSON on standard input and have no timeout, and Ctrl+C
  stops only the hook's own process. Add only hooks you trust.

### Full-screen UI

- On a terminal, `neuron` with no prompt opens a full-screen UI, `neuron-tui`. It is a
  port of the Codebuff/Freebuff terminal UI (Apache-2.0; OpenTUI and React on Bun),
  with Codebuff's services, ads, billing and telemetry removed. It runs the Rust
  `neuron` as its engine over `neuron engine --stdio`. It shows streamed replies,
  tool calls with their output, a permission dialog (allow once, allow for the
  session, deny, deny with a note; Esc denies, Ctrl+C denies and interrupts the turn),
  the Zero-X sign-in with the device code (`/login`), a model picker (`/model`) and a
  footer with the model, git state, tokens and context use.
- Releases ship `neuron-tui-<target>` beside each `neuron-<target>`, with checksums and
  build provenance. The installers and the npm and pip launchers put it beside
  `neuron` (verified, and never failing the install). `--no-tui` or `NEURON_NO_TUI=1`
  keeps the line REPL, and without the UI `neuron` runs the line REPL as before.
- The UI imports an allowed `.envrc` through direnv, minus the variables that would
  reconfigure Neuron: `NEURON_*`, `CLAW_*`, `CLAWD_*` and the engine's other
  compatibility variables (such as the one that sets its default permission mode),
  the providers' keys and endpoints (`OPENAI_*`, `ANTHROPIC_*` and the like), `HOME`,
  `USERPROFILE`, `XDG_CONFIG_HOME`, `GIT_CONFIG*`, and the Node, Bun, TLS and preload settings
  (`NODE_OPTIONS`, `NODE_EXTRA_CA_CERTS`, `SSL_CERT_FILE`, `LD_PRELOAD` and the like).
  The UI's log names what was dropped. The engine is found on the `PATH` from before
  the import.
- The React/Ink prototype in `ui/react-tui` is removed.
- `neuron --continue [CHAT]` reopens a chat in the UI (the latest one for this folder,
  or CHAT) in the chat's own folder and on its own model, also after a turn that was
  interrupted. Chats are kept per project path, chats with only slash commands in them
  are skipped, and the UI keeps its chats, settings and logs in `~/.neuron/tui`
  (`$NEURON_HOME/tui` when `NEURON_HOME` is set). The `--continue` hint printed on exit
  stays on screen.
- NeuronCLI's brand colours: the NEURON logo draws each letter in its colour, as
  `neuron`'s banner does, with blue accents and borders, an orange user line, cyan links
  and inline code, green success and red errors. The light theme uses a darker cyan and
  orange that stay readable on white.
- In a classic Windows console window (Windows PowerShell 5.1 or cmd on Windows 10) the
  UI draws with characters the console fonts have (square corners, `√ ×`, `► ▼ ▲`,
  ASCII for emoji) instead of `?` boxes; `NEURON_TUI_ASCII=1` or `0` overrides the
  choice. The window is titled "Neuron" while the UI runs and gets its own title back at
  exit, and a TrueType font that a PowerShell start had switched to the raster font is
  restored.
- Quitting (`/exit`, Ctrl+C twice) keeps a classic console window and its shell open.
  A command typed right after quitting reaches the shell whole and runs, and `neuron`
  gives the console its input mode back after the UI exits, even when the UI was
  killed, so cmd's Up arrow keeps working.
- While the UI runs, `neuron` watches the terminal and resets it if the UI dies.
- Keys that arrive faster than the screen redraws (a held Backspace, a burst from the
  console, Enter right after the last character) act on the latest text, so quick
  Backspaces delete every character and Enter sends the whole prompt. The chat input
  takes keys even while the terminal last reported that it lost the focus.
- Retry notices appear above the answer, and a retry replaces the failed attempt's
  text. Failed or refused tools say how they ended (exit code, timed out, killed,
  interrupted, blocked) and show their output; an interrupted tool reads "stopped
  (interrupted)". A question from the model (`AskUserQuestion`) is shown in place with
  its answer. Markdown keeps soft line breaks, nested list indentation and Windows
  backslashes.
- `/status` shows the Zero-X account and, in an untrusted folder, what that means and
  how to trust it. The footer shows the account email and drops parts by priority on a
  narrow terminal; an expired or revoked login says so. The model picker opens on the
  current model, greys out unavailable models with the reason and marks degraded and
  busy ones. The sign-in link keeps `https://` and is clickable.
- An engine that crashes is reported once, and the UI then exits with code 1.

### Zero-X gateway and sign-in

- `neuron login` signs in with a device code: it prints
  `https://neuron.zero-x.live/device` and a short code, opens the browser when the
  machine has one (never over SSH, in containers or with `--no-browser` /
  `NEURON_NO_BROWSER`), and polls until the code is approved on that page. No local port
  is opened, so it works over SSH, in WSL and in containers. The answer from the gateway
  is checked before anything is printed (code format, an https link on a Zero-X host),
  and only a CLI session is accepted from it. This replaces the browser callback on a
  loopback port.
- `neuron login --with-token` (reads a `ses_` session or `nzx_` API key from standard
  input), `neuron auth status` (checks the credential with the gateway and prints plan,
  expiry and today's usage; an expired or revoked login reads as signed out and exits
  1) and `neuron logout` (revokes the session on the gateway). `neuron auth
  login|logout` still work. `/login` and `/logout` run the same flows in a session.
  `auth`, `login`, `logout`, `usage` and `upgrade` are found after global flags, and
  `--output-format json` gives JSON results for them.
- The login is kept in the macOS Keychain or Windows Credential Manager, otherwise in
  `~/.neuroncli/credentials.json` (owner-only) with a one-time warning; isolated homes
  (`NEURON_HOME`, a redirected `HOME`/`USERPROFILE`) never touch the OS store, and
  `NEURON_CREDENTIAL_STORE` forces a backend. The old self-decrypting `gateway.enc` is
  migrated once and deleted. A corrupt entry warns and reads as signed out.
- Provider selection: `NEURON_TOKEN`, then `--provider` / `NEURON_PROVIDER` / the
  `provider` setting, then the stored Zero-X login, then provider keys in the
  environment. One startup line names the provider, the credential source and why it
  was chosen. Credentials are handed to the HTTP client in memory and are no longer
  exported into the process environment.
- The model catalog (`GET /v1/models`) is fetched once and cached for 10 minutes per
  credential; `/model` lists it with availability, tiers and context windows, warns
  about unknown or unavailable ids and completes catalog ids.
- The full-screen UI's model list refreshes availability from `/v1/models/status` at
  most every 30 seconds, falling back to the catalog, and a model whose
  `available_again_at` has passed reads as available. The `openai/` routing prefix is
  no longer shown in model names, `/status`, the context report or JSON output.
- `neuron usage` and `/usage` show plan, today's usage with the reset time and
  credits; `/upgrade` opens the pricing page.
- Gateway errors show the message, reset time, upgrade link and request id. Final
  codes (`insufficient_quota`, `insufficient_credits`, `upgrade_required`,
  `region_unavailable`, `account_suspended`, `invalid_session`, `model_not_found`,
  `api_key_not_allowed`) are never retried. `capacity_exhausted` and `rate_limited`
  are retried at most twice, honouring `Retry-After` up to 90 seconds per wait and
  180 seconds in total, with a countdown on the terminal; other transient errors
  (`upstream_*`, `service_unavailable`, 5xx) retry at most twice within 60 seconds.
  An expired or revoked session prints the login instruction once and, in a terminal,
  offers to sign in again.
- `context_length_exceeded` shows the gateway's message, which names the free
  per-minute token budget the prompt went over, and its upgrade link.
  `capacity_exhausted` suggests another model (`/model`, or `--model` in a
  `neuron prompt` run).
- Streaming: parallel tool calls without an `index` (Gemini) are no longer merged into
  invalid JSON, a stream cut off before its finish reason is reported as truncated,
  and tool calls cut off at the output limit are reported instead of retried.
  `reasoning_content` from OpenAI-compatible streams is shown as thinking, not as
  answer text, and output tokens follow the gateway's billable count.

### Smaller requests

- The bundled skills (Ponytail, AntiSlop, gstack, Agent Reach) are listed in the
  system prompt with one line each and a short summary of the always-on rules; the
  Skill tool loads a skill's full text when the model needs it. MCP server tools are
  offered through `ToolSearch` instead of with every request. In a one-file repository
  the system prompt shrank from 66,068 to about 6,200 characters and the first request
  (system prompt plus tool schemas) from about 98,000 to about 17,000 characters
  (roughly 24,500 to 4,500 tokens).
- MCP tool definitions are sent only for the tools the model called or that
  `ToolSearch` found since the last prompt; a request rejected as too large is sent
  once more without them.

### Agent loop and sessions

- A model-aware context manager budgets every request (context window minus output
  reserve, tool schemas and system prompt), caps large tool results, prunes old ones
  and then compacts the history into a structured summary that never splits a tool
  call from its result.
- Transcripts are repaired on load and before every request (unanswered tool calls get
  a synthetic result, orphan results are dropped); malformed session records are
  skipped and counted, and a record is never appended to a cut-off line.
- Ctrl+C cancels the current turn (model stream, tools, hooks and sub-agents) and
  returns to the prompt; a second Ctrl+C quits. A failed turn or slash command no
  longer ends the session. Provider context-overflow errors are recovered by compacting.
- An interrupt also kills a running bash, PowerShell or REPL command with its whole
  process tree and abandons a running MCP call; background commands are killed when
  the session ends. An interrupt right after a prompt is sent cancels that turn.
- An interrupted or failed prompt stays in the conversation as its own turn with a
  short answer in its place ("(no answer: the user stopped this request)", or the
  error), and a stopped partial answer ends with "(answer cut off: ...)". Models no
  longer carry out a stopped request, or answer a failed one, along with the next
  prompt. Only the requests change; the session file is not.
- Stream timeouts: first token 90 seconds (`NEURON_FIRST_TOKEN_TIMEOUT_SECS`), 60
  seconds after a tool result, 120 seconds idle, with one retry. Waits for
  `Retry-After` between retries do not count toward the first-token timeout, and the
  stream retry is one of the provider's retries, so a Zero-X model call sends at most
  three requests.
- A turn makes at most 50 model calls by default (`maxIterations` setting,
  `NEURON_MAX_ITERATIONS`), and repeated identical tool calls are stopped.
- MCP servers start in the background, so the session and its first turn no longer
  wait for `npx -y` installs; their tools join once the servers are up, and
  `ToolSearch` says which servers are still starting or failed. Server output goes to
  `logs/mcp-servers.log` in the Neuron home instead of the terminal. One-shot
  `neuron prompt` runs leave the bundled default servers out.
- Sub-agents use the parent's model, credentials, permission mode, permission rules,
  hooks and tool allow-list; the Agent tool waits for the result (up to
  `NEURON_SUBAGENT_TIMEOUT_SECS`, default 600).
- `neuron --resume <session>` on a terminal continues the session in the REPL, with
  `--model`, `--permission-mode`, `--allowedTools`, `--reasoning-effort` and the other
  launch flags applied whether they come before or after `--resume`, and the
  session's own model when no `--model` is given. `/context` and `/status` show the
  context budget.
- Piped standard input reaches the model with the prompt in every permission mode
  (`git diff | neuron prompt "review this change"`), up to 128 KiB. With a prompt on
  the command line, a pipe that sends nothing for 500 ms is not waited for.
- When standard input is not a terminal, `neuron prompt` asks nothing: a tool call that
  needs approval is denied with a reason the model and the user both see (the model is
  told nothing ran), and a question from the model is refused so it decides itself.
  A run that denied a call ends with a warning naming the denied tools and exits with
  code 2. After 5 denied calls the turn stops, as with Ctrl+C, and says why on standard
  error. The REPL has no such limit.
- Approval, `AskUserQuestion` and workspace trust questions go to standard error when
  standard output is redirected; with standard error redirected too, the call is denied
  (and the question refused) instead of waiting on a prompt nobody can see. In JSON
  mode, standard error carries JSON lines only.
- An unquoted prompt that starts with a subcommand's name (`neuron init the database
  schema`) is refused with a quoting hint instead of running the subcommand; prompts
  that start with words such as help, status or usage go to the model.
- Project instructions are read from `NEURON.md`, `NEURON.local.md`,
  `.neuron/NEURON.md` and `AGENTS.md` as well as the `CLAUDE.md` compatibility files.
- Settings validation accepts the `provider` and `maxIterations` keys.

### Tools and permissions

- Shell command lines (POSIX sh, `cmd.exe` and PowerShell) are classified as a whole:
  a line runs without a prompt in `workspace-write` (and at all in `read-only`) only
  when every command in it is a known read-only program with safe arguments, writes
  only to `/dev/null` and names no path outside the workspace. Searches outside the
  workspace, writes to `.git` or Neuron settings and fetches from a new web domain
  also need approval. An approval given at the prompt is honoured by the tool, and the
  prompt asks in plain words ("Run `npm test`?").
- Workspace trust: a cloned repository's own settings files (`.neuron.json`,
  `.neuron/settings*.json` and the `.claw` equivalents) apply only once you trust the
  directory. Until then only settings that restrict Neuron are kept, and the ignored
  keys are named. A repository's settings files can no longer select a full-access
  mode, even in a trusted folder; use `--permission-mode` or your user settings.
  `NEURON_TRUST_WORKSPACE=1` trusts a checkout in a headless run, and `trustedRoots` in
  your user settings trusts the directories under them. Provider keys are read from
  `./.env` only in a trusted workspace.
- Secrets stay away from the model: reading `.env` files or Neuron settings and
  credentials, printing environment variables (`env`, `printenv`, `echo $VAR`,
  PowerShell `env:`) and recursive searches of a tree that holds such files need
  approval, and directory searches skip them. The system prompt carries only
  `git status` and `git diff --stat` (at most 2,000 characters each) and names the
  settings files, model, permission mode and MCP servers, never setting values. Hook
  results name hooks by event and position. `WebSearch` needs approval once per
  session.
- In an untrusted workspace, the git commands Neuron runs itself switch off the
  repository's fsmonitor and signature checks and skip status and diff when its config
  defines filter drivers. The model's read-only git commands (`status`, `diff`, `show`,
  `grep`, `log -p` and the like) need approval when the repository's own config names
  an fsmonitor, filter, textconv or external diff program. PowerShell paths after a
  bare switch, and `..\` paths on every host, are checked against the workspace.
- Shell commands (bash, PowerShell) and the REPL tool get timeouts (120 seconds by
  default, at most 600), empty standard input and whole-process-tree termination on
  timeout or Ctrl+C; plugin tools get a 120-second timeout that ends their process tree.
  These commands, plugin tools and MCP servers never inherit `NEURON_TOKEN`,
  `NEURON_SESSION_TOKEN`, provider keys such as `OPENAI_API_KEY`, any variable named
  `*_API_KEY`, `*_TOKEN` or `*_SECRET`, or any `ses_`/`nzx_` value (a value set in an
  MCP server's `env` still applies). Hooks are not covered: they run with Neuron's
  environment (see Known limitations).
- On Windows the bash tool runs `cmd.exe /D /S /C` with the command passed verbatim,
  and the PowerShell tool runs `pwsh` (or Windows PowerShell) with an encoded command.
  Settings hooks use the same `cmd` form, so quoted paths with spaces work, and MCP
  servers and hook shells start without a console window, so they no longer retitle
  the terminal.
- WebFetch refuses non-HTTP(S) URLs and private, loopback and link-local addresses,
  pins the validated address, reports redirects to another host instead of following
  them, and returns up to 20,000 characters of readable text.
- `edit_file` keeps the file's line endings, refuses an ambiguous match unless
  `replace_all` is set and returns a short diff; `write_file` returns a summary;
  `read_file` returns at most 2,000 lines with the total; `glob_search` and
  `grep_search` skip `.git`, `node_modules`, build output and `.gitignore`d paths, and
  `grep_search` patterns are regular expressions.
- A bash or PowerShell command that exits non-zero, times out or is killed is reported
  as an error, with its output. A denied call's result tells the model that nothing
  ran.
- Only working tools are offered to the model; bookkeeping and placeholder tools stay
  behind `ToolSearch`. The Config tool cannot change permissions or hooks. MCP
  `tools/call` requests time out after 120 seconds by default, and MCP arguments sent
  as a JSON string are parsed.

### Fixes

- The system prompt names the model in use, as the gateway catalog shows it
  (`GLM 5.2 (glm-5.2)`), instead of telling every model it is "Claude Opus 4.6", and
  heads the project instruction files "# Project instructions". Requests no longer
  carry the `claude-code-20250219` beta. In `/help` and `neuron --help`, `/init` says
  that it writes `NEURON.md` (it already did), and `/config`, `/memory` and `/api-key`
  are described in Neuron's words, as are the errors for unsupported plugin manifest
  fields. The `/agents`, `/skills` and `/mcp` help names `neuron agents`,
  `neuron skills` and `neuron mcp` as the direct commands.
- Text that had been double-encoded (`â–¶ Thinking`, `Â·` separators, `âœ”`) is shown
  correctly again.
- `neuron --help` and `neuron doctor` name this repository and the real install
  channels instead of the old upstream repository and a `cargo install` warning, and
  `--help` lists `--provider` and `--reasoning-effort`.
- Azure turns were counted twice against the local daily quota.
- `/plan off` restores the previous permission mode.
- Requests to the Zero-X gateway no longer carry the non-standard `is_error` field on
  tool results, which made Groq reject every request after the first tool call.
- Code in the terminal keeps its punctuation (`[...values]` was shown as `[…values]`).
- `/login`, `/logout`, `/usage`, `/upgrade`, `/plan` and `/exit` appear in `/help` and
  completion, and `neuron --resume latest /usage` prints the account report.
- Text output keeps its line breaks, paragraphs and the backslashes in paths, prints
  tool output as written instead of as Markdown, and carries no escape sequences or
  banner when written to a file or pipe.
- On Windows, escape-code processing is switched on before anything is printed, and
  characters that classic console fonts lack are replaced with ones they have
  (`√ × ►`).
- Cost estimates are shown only for models with known prices; other models used to be
  estimated at the wrong rates.
- `neuron init` writes `acceptEdits` (workspace-write) instead of `dontAsk`.
- The Verification and statusline-setup sub-agents can load skills, and an interrupted
  sub-agent is reported as `cancelled`.
- Tool output shows Windows paths without the `\\?\` prefix, and `grep_search` and
  `glob_search` take an empty path as the workspace.

### Installation and packaging

- New `install.sh` for Linux (x64, arm64) and macOS (Apple silicon, Intel): downloads
  the release binary and its `.sha256`, verifies it with `sha256sum` or `shasum`,
  checks that it runs, and installs it to `~/.local/bin` or `NEURON_INSTALL_DIR`. It
  warns when the directory is not on `PATH` or another `neuron` shadows it, and it
  replaces the old script that built from source and printed Claw Code text.
- New `install.ps1` for Windows x64, for Windows PowerShell 5.1 and PowerShell 7.
  Installs to `%LOCALAPPDATA%\Programs\Neuron\bin`; changes the user `PATH` only with
  `-AddToPath`, and warns when another `neuron` comes first on `PATH` (an old
  NeuronCLI folder or npm or pip package, say) with how to remove it. Under
  `irm | iex` it leaves the caller's session settings alone. It
  hashes downloads with .NET's SHA-256, so it also works when Windows PowerShell 5.1
  inherits a PowerShell 7 module path without `Get-FileHash`.
- Both installers accept a version (`NEURON_VERSION`, default: the latest stable
  release) and a mirror (`NEURON_DOWNLOAD_BASE`), reject non-HTTPS mirrors (plain HTTP
  only to an exact loopback host, never with user info in the URL), and tell an
  unpublished version apart from a network failure. The commands they print (for the
  other platform's installer, or to rerun with `-AddToPath`) name the requested
  version. `GITHUB_TOKEN` is sent only to `api.github.com`, and `install.sh` keeps it
  off the command line.
- `install.sh` checks that the binary runs from the install directory, so a `/tmp`
  mounted `noexec` does not break it; `install.ps1` puts the previous `neuron.exe`
  back if the new one cannot be moved into place. Both print the installed version.
- The Python launcher (`neuroncli`) verifies the binary once when it downloads it,
  stores a `.sha256` next to it and runs it from the cache afterwards with no network
  access. Download problems print an actionable message instead of a traceback.
  Ctrl+C reaches Neuron instead of killing it, the exit code is passed through, and on
  Linux and macOS the launcher replaces itself with the binary. Prerelease versions
  download from their own tag (`v7.0.0-alpha.1`).
- The npm launcher (`@zero-x.live/neuron`) downloads and verifies the binary on first
  run when install scripts were skipped (`--ignore-scripts`), falls back to the user
  cache when the package directory is read-only, forwards SIGTERM and SIGHUP (and
  SIGINT that did not come from the terminal) to Neuron, and exits with Neuron's code.
  A download fails only when no data arrives for 60 seconds, so slow links work.
- The PyPI wheel is a pure-Python launcher; it never contains a native binary.

### Release pipeline

- Release binaries for linux-x64, linux-arm64, macos-arm64, macos-x64 and windows-x64,
  each with a `.sha256` file, plus `SHA256SUMS`, `THIRD_PARTY_NOTICES.md` and both
  install scripts, with GitHub build provenance attestations. The Linux binaries are
  built on Ubuntu 22.04 and need glibc 2.35 or later.
- Before anything is published to npm or PyPI, the workflow HEAD-checks every public
  asset URL, verifies the checksums and installs from the release on Linux, macOS and
  Windows. The release becomes "Latest" only after those checks pass, and a prerelease
  such as this one never does; npm publishes prereleases under the `next` dist-tag.
  npm and PyPI publishing stay off unless explicitly enabled, and the jobs that hold
  the publishing token install nothing and upload only the files that were tested.
- The install commands in the release notes name the release's version, so they work
  for a prerelease too.
- The assets of a published release are never rebuilt or replaced; a later run on the
  same tag only verifies them and publishes the packages.

### Licensing

- Added `LICENSE` (MIT), keeping the Claw Code copyright notice alongside Neuron's,
  and `NOTICE` describing where the code comes from, the Apache-2.0 origin of the
  full-screen UI and the licenses of the bundled skills. Both ship in the npm package
  and the Python wheel.
- The full-screen UI in `ui/neuron-tui` keeps Freebuff's Apache-2.0 `LICENSE` and
  `NOTICE` unchanged, and every file changed from its Freebuff original says so in a
  header.
- `scripts/generate-third-party-notices.mjs` builds `THIRD_PARTY_NOTICES.md` (crate,
  version, license, repository and license texts) from `cargo metadata`, and adds the
  UI's own license and notice files and the npm packages compiled into `neuron-tui`.

### Documentation

- Rewrote the README, usage guide and release runbook for the Neuron product, and added
  a security policy, contributing guide, code provenance summary and an issue triage
  note. The security policy describes where the login is stored (OS credential store,
  or an owner-only file) and what a leaked credential means. The usage guide describes
  the full-screen UI, workspace trust and how `neuron prompt` behaves in a script.

## 6.2.5 and earlier

Versions up to 6.2.5 were built outside a reproducible release pipeline. The npm 6.2.0
and PyPI 6.2.4 (and earlier) packages do not match this repository's release binaries
and are not supported; upgrade to 7.0.0-alpha.1 or later.
