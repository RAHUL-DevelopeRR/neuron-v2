# Changelog

Notable changes to Neuron. Versions follow [semantic versioning](https://semver.org/).
The release workflow copies a version's section into its GitHub release notes.

## 6.3.0 (unreleased)

The first Neuron release built and published by this repository's release pipeline.
It signs in to the Zero-X gateway and keeps the login in the OS credential store,
makes the agent loop survive errors, cancellation and long sessions, gates every tool
call by what it actually does, and sends far smaller requests.

### Zero-X gateway and sign-in

- `neuron auth login` (browser callback on a random loopback port, `--no-browser`
  prints the URL), `neuron auth login --with-token` (reads a `ses_` session or `nzx_`
  API key from standard input), `neuron auth status` (checks the credential with the
  gateway and prints plan, expiry and today's usage) and `neuron auth logout` (revokes
  the session on the gateway). `/login` and `/logout` run the same flows in a session.
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
- Streaming: parallel tool calls without an `index` (Gemini) are no longer merged into
  invalid JSON, a stream cut off before its finish reason is reported as truncated,
  and tool calls cut off at the output limit are reported instead of retried.
- `docs/GATEWAY_CONTRACT.md` mirrors the platform contract v1.2.

### Smaller requests

- The bundled skills (Ponytail, AntiSlop, gstack, Agent Reach) are listed in the
  system prompt with one line each and a short summary of the always-on rules; the
  Skill tool loads a skill's full text when the model needs it. MCP server tools are
  offered through `ToolSearch` instead of with every request. In a one-file repository
  the system prompt shrank from 66,068 to about 6,200 characters and the first request
  (system prompt plus tool schemas) from about 98,000 to about 17,000 characters
  (roughly 24,500 to 4,500 tokens). Tests keep the default system prompt under 10,000
  characters.

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
- Stream timeouts: first token 90 seconds (`NEURON_FIRST_TOKEN_TIMEOUT_SECS`), 60
  seconds after a tool result, 120 seconds idle, with one retry.
- A turn makes at most 50 model calls by default (`maxIterations` setting,
  `NEURON_MAX_ITERATIONS`), and repeated identical tool calls are stopped.
- Sub-agents use the parent's model, credentials, permission mode, permission rules,
  hooks and tool allow-list; the Agent tool waits for the result (up to
  `NEURON_SUBAGENT_TIMEOUT_SECS`, default 600).
- `neuron --resume <session>` on a terminal continues the session in the REPL, with
  `--model`, `--permission-mode`, `--allowedTools`, `--reasoning-effort` and the other
  launch flags applied whether they come before or after `--resume`, and the
  session's own model when no `--model` is given. `/context` and `/status` show the
  context budget.
- Project instructions are read from `NEURON.md`, `NEURON.local.md`,
  `.neuron/NEURON.md` and `AGENTS.md` as well as the `CLAUDE.md` compatibility files.
- Settings validation accepts the `provider` and `maxIterations` keys.

### Tools and permissions

- Shell command lines (POSIX sh, `cmd.exe` and PowerShell) are classified as a whole:
  a line runs without a prompt in `workspace-write` (and at all in `read-only`) only
  when every command in it is a known read-only program with safe arguments, writes
  only to `/dev/null` and names no path outside the workspace. Searches outside the
  workspace, writes to `.git` or Neuron settings and fetches from a new web domain
  also need approval. An approval given at the prompt is honoured by the tool.
- Tool subprocesses (bash, PowerShell, hooks, plugin tools, MCP servers) get timeouts
  (120 seconds by default for commands, at most 600), empty standard input and
  whole-process-tree termination on timeout or Ctrl+C, and never inherit
  `NEURON_TOKEN`, `NEURON_SESSION_TOKEN` or any `ses_`/`nzx_` value.
- On Windows the bash tool runs `cmd.exe /D /S /C` with the command passed verbatim,
  and the PowerShell tool runs `pwsh` (or Windows PowerShell) with an encoded command.
- WebFetch refuses non-HTTP(S) URLs and private, loopback and link-local addresses,
  pins the validated address, reports redirects to another host instead of following
  them, and returns up to 20,000 characters of readable text.
- `edit_file` keeps the file's line endings, refuses an ambiguous match unless
  `replace_all` is set and returns a short diff; `write_file` returns a summary;
  `read_file` returns at most 2,000 lines with the total; `glob_search` and
  `grep_search` skip `.git`, `node_modules`, build output and `.gitignore`d paths, and
  `grep_search` patterns are regular expressions.
- Only working tools are offered to the model; bookkeeping and placeholder tools stay
  behind `ToolSearch`. The Config tool cannot change permissions or hooks. MCP
  `tools/call` requests time out after 120 seconds by default.

### Fixes

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

### Installation and packaging

- New `install.sh` for Linux (x64, arm64) and macOS (Apple silicon, Intel): downloads
  the release binary and its `.sha256`, verifies it with `sha256sum` or `shasum`,
  checks that it runs, and installs it to `~/.local/bin` or `NEURON_INSTALL_DIR`. It
  warns when the directory is not on `PATH` or another `neuron` shadows it, and it
  replaces the old script that built from source and printed Claw Code text.
- New `install.ps1` for Windows x64, for Windows PowerShell 5.1 and PowerShell 7.
  Installs to `%LOCALAPPDATA%\Programs\Neuron\bin`; changes the user `PATH` only with
  `-AddToPath`. Under `irm | iex` it leaves the caller's session settings alone.
- Both installers accept a version (`NEURON_VERSION`, default: latest release) and a
  mirror (`NEURON_DOWNLOAD_BASE`), reject non-HTTPS mirrors (plain HTTP only to an
  exact loopback host, never with user info in the URL), and tell an unpublished
  version apart from a network failure. `GITHUB_TOKEN` is sent only to
  `api.github.com`, and `install.sh` keeps it off the command line.
- `install.sh` checks that the binary runs from the install directory, so a `/tmp`
  mounted `noexec` does not break it; `install.ps1` puts the previous `neuron.exe`
  back if the new one cannot be moved into place. Both print the installed version.
- The Python launcher (`neuroncli`) verifies the binary once when it downloads it,
  stores a `.sha256` next to it and runs it from the cache afterwards with no network
  access. Download problems print an actionable message instead of a traceback.
  Ctrl+C reaches Neuron instead of killing it, the exit code is passed through, and on
  Linux and macOS the launcher replaces itself with the binary. Prerelease versions
  download from their own tag (`v6.4.0-rc.1`).
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
  Windows. The release becomes "Latest" only after those checks pass. npm and PyPI
  publishing stay off unless explicitly enabled, and the jobs that hold the publishing
  token install nothing and upload only the files that were tested.
- The assets of a published release are never rebuilt or replaced; a later run on the
  same tag only verifies them and publishes the packages.
- New Packaging CI runs the launcher, installer and npm tarball tests against a local
  fake release on Linux, macOS and Windows for every pull request that touches them.

### Licensing

- Added `LICENSE` (MIT), keeping the Claw Code copyright notice alongside Neuron's,
  and `NOTICE` describing where the code comes from and the licenses of the bundled
  skills. Both ship in the npm package and the Python wheel.
- `scripts/generate-third-party-notices.mjs` builds `THIRD_PARTY_NOTICES.md` (crate,
  version, license, repository and license texts) from `cargo metadata`.

### Documentation

- Rewrote the README, usage guide and release runbook for the Neuron product, and added
  a security policy, contributing guide, code provenance summary and an issue triage
  note. The security policy describes where the login is stored (OS credential store,
  or an owner-only file) and what a leaked credential means.

## 6.2.5 and earlier

Versions up to 6.2.5 were built outside a reproducible release pipeline. The npm 6.2.0
and PyPI 6.2.4 (and earlier) packages do not match this repository's release binaries
and are deprecated; upgrade to 6.3.0 or later.
