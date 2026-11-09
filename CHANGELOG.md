# Changelog

Notable changes to Neuron. Versions follow [semantic versioning](https://semver.org/).
The release workflow copies a version's section into its GitHub release notes.

## 7.0.0-alpha.6 (unreleased)

- Windows installer: `irm <release>/install.ps1 | iex` now installs that release and adds
  it to your user `PATH` with no extra parameters (`-NoPathChange` or
  `NEURON_NO_MODIFY_PATH=1` opts out; `-AddToPath` is still accepted). Downloads use
  `curl.exe` when Windows has it (with retries) and fall back to .NET, and show their size,
  percentage, speed and time left instead of a frozen line. `neuron.exe` is installed and
  announced before the 110 MB full-screen UI download starts, and the UI's start-up check
  is limited to 20 seconds (a first run that Windows Defender holds up longer is not a
  failure). Both checksum checks and the `neuron --version` check are unchanged. The final
  message says to open a new terminal and run `neuron login`.
- `install.sh` and `install.ps1` no longer look up GitHub's `releases/latest`, which is a
  404 while every release is a prerelease: the release workflow stamps each release's
  version into its scripts, and an unstamped script takes the newest non-draft release
  from the release list. `install.sh` shows a progress bar on a terminal and treats a
  `curl` exit status of 0 without a 2xx response as a failure (curl 8.14 does that after
  `--retry`).
- npm on Windows: the README and the postinstall output explain PowerShell's "running
  scripts is disabled" error (use `npm.cmd` and `neuron.cmd`, Command Prompt, or the
  PowerShell installer). The README says that Windows has no Python by default, and that
  the pip package needs Python 3.9 or later.

## 7.0.0-alpha.5 (2026-11-07)

- Trust hardening: the npm and PyPI launchers now check the downloaded binary against the
  release `SHA256SUMS` (from the GitHub release even when `NEURON_DOWNLOAD_BASE` is a
  mirror; `NEURON_CHECKSUMS_BASE` overrides, HTTPS or loopback only) and fail closed when it
  is missing, empty, does not list the file or disagrees. A cached binary whose size or
  modification time changed since it was verified is hashed again and not run on a mismatch.
  `install.sh` refuses redirects away from HTTPS.
- Trust hardening: folder trust now covers every project setting that applies once trusted
  (`env`, `model`, plugins, `apiKeyHelper`, a non-restrictive permission mode, ...), not only
  hooks, MCP servers and allow rules. Changing one asks again, and the trust screen names the
  keys that changed; settings that only restrict do not ask. What is trusted is what the
  screen showed: files changed while it was open are not trusted, and it is shown again. A
  command longer than 2000 characters is cut with a note giving its length and the file that
  holds all of it. Folders trusted before this release ask once more.
- Trust hardening: a session approval of `pwsh`, `powershell` or `cmd` no longer covers other
  calls (`-Command`, `-EncodedCommand`, `/c` and `/k` ask every time; `-File x.ps1` covers
  that script), the same for `pypy`, `npx`, `bunx`, `uv run` and `npm exec`;
  `jq -n -- env` is not read-only; the plugin and MCP child environment drops `PGPASSWORD`,
  `MYSQL_PWD`, `REDIS_URL`, `KUBECONFIG` and names ending in `_DSN` or `_URI`.
- `WebSearch` goes through the Zero-X gateway (`POST /v1/search`) when you are signed in,
  with the same sign-in as chat, and shows how many of the day's searches are left. It
  takes an optional `freshness` (`day`, `week`, `month`, `year`) and still honours
  `allowed_domains` and `blocked_domains`. When the gateway answers 503, cannot be
  reached, rejects the sign-in, or you are signed out, it falls back to the local
  DuckDuckGo search and marks the answer "local fallback" (that search cannot filter by
  date). When the day's quota is used up it says so and does not scrape instead.
  Approval is the same as before.
- Trust screen, MCP: each project server shows what it runs (command line or URL,
  secret-looking values redacted) before you tick it, and the approval is bound to
  what was shown: `trust_preview` servers carry `command` and `fingerprint`, and
  `approve_mcp_server` takes the `fingerprint` back and is refused if the config
  changed since. Ticked servers are approved only by an engine in the folder they were
  shown for; a failed restart or a switch to another folder drops them, and an engine
  that cannot approve them says they still wait.
- Full-screen UI: in a terminal too small for it, the note on stderr says what runs
  instead (the line REPL, or a plain-text answer for `neuron "prompt"`), and for
  `--continue` asks for a bigger window. While "Terminal too small" shows, keys and
  pastes are dropped (Ctrl+C still works), so a hidden permission prompt or the
  composer is never answered unseen.
- Full-screen UI: in a terminal that draws East Asian ambiguous-width characters two
  columns wide (a Chinese, Japanese or Korean locale, or `NEURON_AMBIGUOUS_WIDE=1`),
  the status bar measures its `│ ● ○ ◑` glyphs as two columns, so it still fits its row.
- Full-screen UI, MCP panel: a server that waits for you carries a badge after its
  name (`[blocked]`, `[needs approval]`, `[needs connect]`, `[unpinned]`), and its
  summary no longer repeats it.
- Full-screen UI, MCP: a connectors chip in the status bar (`⧉ 5/6`, amber when a
  server needs you, a lock for a policy-required one; click opens the panel). The MCP
  panel gains an Installed tab (approve, connect, sign in, remove) and an Add tab
  (search the catalog, masked secret fields, user/project/local scope). The trust
  screen lists the project's MCP servers to approve at once. MCP tool calls name their
  connector and the tool's catalog tier. The engine sends the servers again once its
  background start finishes, and `McpServerInfo.tool_tiers` carries the tiers.
- Full-screen UI: the agent's questions open a question form (pick a choice with ↑/↓
  and Enter, or write your own answer) instead of a plain notice. The UI declares the
  `questions` capability and answers with the `answer` command; Esc leaves the question
  to the input, as before.
- Full-screen UI: the status bar shows the Free token window as used/limit with its
  reset time (`Free · 5h 640K/1M · resets 14:20 · 37/100 req`; `640K/1M` when narrow),
  amber from 80% and red from 95%. Past its reset time the stale figures are left out.
- Full-screen UI: a terminal smaller than 60x16 gets a "Terminal too small" screen
  (the chat stays open underneath), and `neuron` starts the line REPL in one with a
  note. The status bar, the MCP panel and the model picker measure text in terminal
  columns, so wide characters in a branch, email or model name no longer push the line
  onto the next row.
- Hosted connectors (off until `NEURON_HOSTED_CONNECTORS=1`): `"type": "zerox"` MCP
  servers reach a connector linked on the Zero-X dashboard through the gateway's
  `/mcp/<id>` with your Zero-X sign-in, and each hosted connector the plan's policy
  lists appears as a `managed` server. One that is not linked yet shows
  `needs_connect` (`McpServerInfo.connect_url`, engine feature `mcp_hosted`);
  `/mcp connect <server>` and `c` in the MCP panel open the page that links it; once
  linked, it joins the next turn (or the MCP panel when it opens) without a restart.
  Neuron asks you about every call of a hosted connector's destructive tool, even in
  full access or with an allow rule, and tells the gateway only about the call you
  approved (`-p` in a script does not run them). Older settings for these servers keep working;
  `/mcp` and the protocol call the transport `zerox`.
- MCP supply chain: catalog connectors and the default MCP servers no longer start
  through `npx -y` or `uvx`. The catalog (`2026-11-05.3`) pins each local entry's whole
  npm dependency tree with `sha512` integrity, its Python requirements with wheel
  hashes, or its release binary's `sha256`; the first start installs into
  `~/.neuron/mcp/<id>/<version>` and checks every hash (npm packages are unpacked by
  Neuron, so no install script runs; Python installs are wheels only with
  `--require-hashes`), later starts are offline, and a changed installed file or cached
  tarball stops the start with an integrity error. Codebase memory runs its
  hash-checked release binary; the npm package no longer installs `codebase-memory-mcp`,
  whose install script downloaded that binary without a pinned hash. `/mcp list`, its
  JSON and the MCP panel mark other stdio servers `unpinned` (`McpServerInfo.unpinned`),
  and `/mcp pin <server>` records their version and hashes.
- Verified MCP installs take a lock file per server folder, so a second session that
  starts the same server waits for the first install instead of deleting a Python
  virtual environment that is still being built.
- A verified Python MCP install now records every file of its virtual environment, so a
  changed module stops the start with an integrity error, as a changed npm file does.
- `/mcp pin` of a program (not `npx` or `uvx`) holds only for the files it was pinned
  with: the same relative launch (`node server.js`) in another workspace is unpinned
  there, instead of borrowing the first workspace's pin.
- npm package: `@zero-x.live/neuron` no longer depends on `@playwright/mcp`. npm does
  not hold a published package to its `package-lock.json`, so that tree was whatever
  npm resolved and Neuron never checked it; Playwright now starts from its verified
  install for npm users too, with the launcher's Node.js, and
  `NEURON_PLAYWRIGHT_MCP_ENTRY` is ignored.
- Memory connector: its graph is kept in `~/.neuron/mcp-data/memory/memory.jsonl`
  (catalog `2026-11-05.4`), which its sandbox may write, instead of its package folder,
  where a reinstall would delete it and the sandbox did not let it write.

- MCP sandbox: every stdio MCP server runs with a sandbox profile from its
  `mcpServers.<name>.sandbox` setting (`fsRead`, `fsWrite`, `network`, `envAllow`), its
  catalog entry, or the default for unvetted servers (the workspace read-only, no
  network). Its environment is an allowlist that never passes Zero-X or provider
  credentials, and its network goes through a local proxy for its own host list. On
  Windows a kill-on-close job object ends the server's whole process tree with it, so no
  `node.exe` is left behind; macOS enforces the profile with `sandbox-exec` and Linux
  with bubblewrap when installed. Workspace settings cannot turn the sandbox off, a
  changed `sandbox` setting needs approval again, and `/mcp show` prints the profile.
- MCP sandbox fixes: on Linux a server is no longer killed when the thread that
  started it ends (bubblewrap's `--die-with-parent` is gone), gets a private `/tmp`,
  and no longer sees `/run/user`, `/run/dbus` or `XDG_RUNTIME_DIR`; the shared npm and
  uv caches are writable only by a server that may reach that registry, others get a
  cache of their own; only a server's command and the script its interpreter runs are
  readable beyond its profile (listed in `/mcp show`), not every path in its
  arguments; Codebase memory may write its package's `bin` folder (catalog
  `2026-11-05.2`); Node servers follow the sandbox proxy in `fetch`
  (`NODE_USE_ENV_PROXY`); on Windows a server that could not be put in its job object
  is stopped with `taskkill /T`; and the managed settings' `mcpPolicy.requireSandbox`
  ignores `"sandbox": false` outside them.
- Connector policy: machine-wide managed settings (`managed-settings.json` in
  `C:\Program Files\Neuron`, `/Library/Application Support/Neuron` or `/etc/neuron`,
  read only when no one but an administrator can have written the file or its folders)
  load last and cannot be overridden; their `mcpPolicy` allows, denies and requires MCP
  servers, limits stdio servers, unvetted servers and remote hosts, adds tool rules
  (deny always wins) and can fail closed. The Zero-X plan's `policy.connectors` from
  `/api/account` (contract v1.3) is cached in `~/.neuron/policy-cache.json` and applies
  after a restart and offline. Refused servers show as `blocked` with the reason;
  required ones cannot be turned off or removed. Every MCP call, refused ones too, is
  logged metadata-only to `~/.neuron/audit/mcp-YYYY-MM-DD.jsonl`. The engine protocol
  adds the `managed` scope and `McpServerInfo.required` (feature `mcp_policy`).
- Connector policy fixes: a deny rule in user or workspace settings no longer blocks a
  required server (only the managed settings' own rules do); deny lists also match a
  connector's package at another version and its remote host; a cached plan policy is
  never current for more than a day after it was read; MCP resource list and read
  requests are audited too.
- Connector catalog: `/mcp catalog`, `/mcp add <id> [--scope user|project|local]` and
  `/mcp remove <server>` (and `neuron mcp catalog|add|remove`) list and add vetted MCP
  servers (GitHub, Filesystem, Git, Fetch, Memory, Playwright, Codebase memory,
  Supabase, Context7, Cloudflare docs, AWS documentation) at pinned versions.
  `neuron mcp add` asks for secrets with the input hidden; secrets go to the OS
  keychain and settings hold `${secret:NAME}`. Catalog connectors get the permission
  tier their entry pins per tool. The engine protocol adds `list_connector_catalog`,
  `add_mcp_server`, `remove_mcp_server`, `approve_mcp_server` and the
  `connector_catalog` event (feature `mcp_catalog`).
- Connector catalog fixes: a resumed `/mcp add` or `/mcp remove` runs once for its text
  and JSON; a server name can be added in one settings scope only, and removing one
  keeps the secrets and sign-in another scope's server of that name uses; Memory and
  Codebase memory writes need full access (they write outside the workspace); a stdio
  secret is bound to the server's environment too; an OAuth connector added through
  the engine signs in from there; a signed catalog update must be newer than the
  catalog in use; Filesystem and Memory are listed as `Apache-2.0 AND MIT`.
- MCP hardening: a call through `MCPTool` is checked as the tool it names, so a deny
  or ask rule on `mcp__<server>__<tool>` can no longer be stepped around, and rules can
  name a whole server (`mcp__<server>`, `mcp__<server>__*`); a server a deny rule
  refuses whole shows as `blocked` and does not start. Servers other than the bundled
  ones cannot mark their own tools read-only. A workspace's servers, including those in
  a project `.mcp.json`, wait at `needs approval` until `/mcp approve <server>`, and
  again when their launch spec changes. MCP output reaches the model enveloped as
  external content, cleaned of control characters, redacted and capped at 25,000
  characters with the rest in a spill file. `neuron mcp serve` checks every call
  against your permission rules (read-only unless `--permission-mode` is given),
  annotates its tools, and refuses an untrusted folder.
- MCP hardening follow-ups: near-JSON `MCPTool` calls and `MCPTool` calls that name the
  tool with `tool` or carry extra keys meet the named tool's rules; an approval also
  covers environment and header values and the `oauth` block (existing approvals are
  asked again once); approving a disabled workspace server records it; a server can
  no longer fake the end of its external-content block; spill files go to
  `~/.neuron/mcp-output` inside the envelope; and the tool descriptions and schemas of
  servers Neuron does not ship are cleaned and capped.
- Remote MCP servers: a server with a `url` is reached over Streamable HTTP (or SSE
  with `"type": "sse"`), and `neuron mcp login <server>` signs in to one that needs
  OAuth; the token is kept in the OS keychain, or in an owner-only file when no
  keychain is available. `neuron mcp enable|disable <server>` and `/mcp enable|disable`
  turn servers on and off, and the full-screen UI has an MCP panel.
- A request to create or change files that a model answers in chat, without tools, is
  nudged once and then retried on a tool-capable model; a provider that stays down
  moves the turn to the next `providerFallbacks` model. `toolCapableModels`,
  `autoToolModel`, `subagentModel`, `summaryModel` and `reviewerModel` pick models for
  particular jobs.
- Token-shaped secrets in tool output, the session file and telemetry are replaced
  with `[REDACTED:<kind>]` (`"redactSecrets": false` turns it off).
- Sandbox: on Windows, shell commands run in a job object that kills what they start
  when they end, with `sandbox.maxProcesses` and `sandbox.memoryLimitMb` limits; the
  macOS sandbox uses a seatbelt profile that keeps writes inside the workspace and
  temporary folders; `sandbox.network.allow`
  limits the hosts `WebFetch` and shell commands may reach.
- A nightly eval workflow gates on the coding-task pass rate and drives the
  full-screen UI through a scripted prompt, tool call, approval and answer.
- Two runs started in the same millisecond no longer get the same session and refuse
  each other: session ids are `session-<millis>-<pid>-<rand>`, and a new session that
  finds its id taken picks another.
- `--resume <file>` no longer leaves a `<file>.lock` beside the file: session locks are
  kept in `~/.neuroncli/locks` (under `NEURON_HOME` if set), and a file that is not a
  session is refused before it is locked. A `.jsonl.lock` an older release is holding
  is still honoured for this release.
- `--output-format json` for `--resume <session> -p` reports that run's `usage` and
  `estimated_cost` instead of the session's totals, which are in a new `session_usage`.
- `echo /status | neuron --resume latest -p` runs the piped slash commands as
  `--resume latest /status` does instead of sending them to the model, and
  `echo /status | neuron -p` is refused as piped slash commands are for `neuron`.
- `NEURON_AUTO_COMPACT_INPUT_TOKENS` sets the automatic compaction threshold; the
  inherited variable name it replaces still works.
- A `.clawd-todos.json` an earlier release left in the workspace is removed once the
  task list is saved in Neuron's home, unless git tracks it or fails to say inside a
  repository (a corrupt index, a folder refused for dubious ownership).
- `/rewind` works in the line REPL (`--no-tui`) as in the full-screen UI: a checkpoint
  is saved with each prompt, and `/rewind <id> [code|chat|both]` goes back to one.
  `/help` and Tab completion list it, and `neuron /rewind` says to run it in a session.
- On a terminal, `neuron "prompt"` (without `-p`) opens the full-screen UI and sends the
  prompt first, and `neuron --resume SESSION` opens it on that session, showing its
  conversation (from the home folder or a drive root without `--allow-broad-cwd`, the
  line REPL continues it, asking first as before); `-p` and `neuron prompt` stay plain
  text for scripts. A `-p` run on a terminal starts with one line on stderr (model,
  permission mode, plan) instead of the boxed banner, and no longer walks the
  workspace to draw it.
- In `workspace-write` mode, build, test, run and package-install commands that stay
  in the workspace (`npm install`, `npm run dev`, `npx`, `node`, `cargo build`/`test`,
  `python -m`, `pip`/`uv` installs, `go build`/`test`, `make`, ...) run without asking,
  each part of a `&&`/`|` line judged on its own; sudo, global installs, `curl | sh`,
  paths or writes outside the workspace and `git push` still ask, and so do inline
  code (`node -e`, `python -c`, `echo ... | node`), wrappers that start a program off
  the list (`uv run git push`, `pnpm exec ...`) and writes into `.git`, Neuron settings
  or package-manager configuration such as `.npmrc`. The approval says a command is
  outside the auto-allow list instead of asking for full access.
- Permission prompts offer "always allow in this project" (`p`), which saves a rule as
  wide as the session choice (`bash(cargo install:*)`) to `.neuron/settings.local.json`
  and is not offered for `sudo`, `... | sh` or a `cd` out of the workspace, and "deny
  for this session" (`d`), after which commands that would ask and start with the same
  prefix are refused without asking. A denied model is told plainly not to retry the
  command or a variant.
- The Free plan's rolling 5-hour token window is shown in `/usage`, `neuron usage` and
  a new `Plan` section of `/status` (`Free 640K/1M, resets 14:20 UTC`), kept current
  from each reply's quota headers, and Neuron warns once at 90% of it; the near-limit
  upgrade hint follows the window instead of the daily tokens, which it does not refuse.
  Past the window's reset time the old figures count as unknown until the account is
  read again. The full-screen UI protocol gains the window
  (`AccountDetails.usage_window`, every part optional) and an estimate of each
  request's size (`Status.request_tokens_estimate`).
- On a plan with a token window, Neuron keeps a working set of about 40,000 tokens
  (`workingSetTokens`, `NEURON_WORKING_SET_TOKENS`): older tool results become short
  stubs from half of it, and it asks before a request would need more tokens than the
  window has left (a script is warned instead; a sub-agent stops). `neuron prompt`
  reads the window too. On the gateway, requests carry a `prompt_cache_key` (a hash of the
  session id), and `auto` requests that attach tools carry `x-neuron-route: agentic`
  so the gateway picks a model that calls tools well.

## 7.0.0-alpha.4 (2026-11-03)

The fourth Neuron alpha. In the full-screen UI, the line under the input is now a
status bar: the model, its thinking level, the folder and git branch, the permission
mode and how full the context window is, each a click or a key away from a picker,
with your Zero-X plan and usage on the right. The status bar is the only change in
this release: the bug fixes and the MCP work made since 7.0.0-alpha.3 are not part of
it.

It is still a GitHub prerelease. The [known limitations of 7.0.0-alpha.1](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/v7.0.0-alpha.1/CHANGELOG.md#known-limitations)
still apply, except the ones about npm and PyPI and the ones earlier alphas fixed.

### npm and PyPI

- The npm and PyPI packages are published after this GitHub release, once its assets
  have been checked, so they can appear later than the release itself.
- On npm this alpha is published under the default `latest` dist-tag. When
  `npm view @zero-x.live/neuron version` prints `7.0.0-alpha.4`,
  `npm install --global @zero-x.live/neuron` installs it (run the same command to
  upgrade from 7.0.0-alpha.3); while it still prints `7.0.0-alpha.3`, use the install
  scripts.
- On PyPI the alpha is a pre-release (`7.0.0a4`), installed with
  `pip install --pre neuroncli` (`pip install --upgrade --pre neuroncli` to upgrade)
  once `pip index versions neuroncli --pre` lists it.
- The old 6.2.x builds (npm 6.2.0, PyPI 6.2.4 and earlier) remain unsupported.

### Full-screen UI: the status bar

- The bar has five parts, left to right: the model, the thinking level, the folder and
  branch, the permission mode and the context meter. Click a part, or press Alt+M
  (model), Alt+T (thinking level), Alt+W (folder), Alt+P (permission mode) or Alt+C
  (context), to open its picker; on macOS the keys are Option+M and so on once the
  terminal is set to use Option as Meta (Alt), otherwise click the part. The help
  screen lists them under "Status bar". The session's token counts, which the old
  footer showed, are in the context popover.
- In a narrow terminal, parts give way in a fixed order (the account shortens first,
  then the labels, the git counts, the folder, half the meter's circles and the account
  go, and so on). The provider letter, the permission mode and the context percentage
  always stay.

### Model and thinking level

- The model part shows a letter for the provider and the model's name. With `auto` it
  reads `auto → <model that answered>`, and a model your plan can no longer use reads
  `(locked)`.
- In the model picker, Tab on a model lists its thinking levels; Enter on a level
  switches to that model and sets that level.
- The thinking level is the one the next turn asks for, from the levels the model
  offers. Pick it from the bar (Alt+T), or type `/effort <level>`. `/effort` alone
  shows the current level and the model's levels, and `/effort default` goes back to
  the model's own default. `/effort` used to be refused in the full-screen UI.
- Switching to a model that does not take the current level goes back to that model's
  default and says so ("Thinking level: default. This model does not take ...").

### Folder and branch

- The folder part opens your recent folders. Picking one restarts Neuron there with a
  new session, and the chat gets a line saying the new session is in another folder.
  While a turn runs, or waits for your approval or an answer, the picker asks for a
  second Enter first, because switching stops that turn.
- The branch part shows the branch with its staged (`+`), unstaged (`~`), untracked
  (`?`) and conflicted (`!`) file counts. Clicking it lists the local branches, also
  while a turn runs; switching takes a second Enter.
- `/branch` lists the local branches, `/branch <name>` switches to one and
  `/branch new <name>` creates one and switches to it. A switch is refused while
  tracked files have uncommitted changes ("Commit or stash your changes before
  switching").

### Permission mode

- The permission part shows Read-only, Edit workspace or Plan; Full access is offered
  only when Neuron was started with it. Shift+Tab cycles them (Read-only, Edit
  workspace, Full access when allowed, Plan); Alt+P or a click lists them.
- A mode chosen during a turn takes effect after the turn. Until then the bar shows it
  as pending (for example "Edit workspace → Plan next") and the picker marks it. Shift+Tab moves
  on from the pending choice, so two quick presses go two steps instead of sending the
  same choice twice.

### Context meter and popover

- The meter is ten circles (five when narrow), each a tenth of the usable context
  window (the window less the space kept for the reply), with partly filled circles and
  the percentage. It is green, then the normal text colour from half full, the warning
  colour from 80% and the error colour from 95%. The circle where auto-compaction
  starts is marked, and `~` before the percentage means it is an estimate.
- Alt+C or a click opens the context popover: what fills the window (system prompt,
  tools, project instructions and memory, conversation, tool outputs, free), the
  session's tokens, and your Zero-X account's 5-hour window and today's requests with
  when each resets. The account rows are read again every minute while it is open. In
  a short terminal the popover keeps the meter, the free share, the auto-compaction
  line and the account title, and the status bar stays on screen.
- `/context` in the full-screen UI opens the same breakdown as a report, with the
  usable tokens, the space kept for the reply and where auto-compaction starts.

### Zero-X plan and usage

- The right side shows your plan, the share of its 5-hour token window you have used
  (`Free · 5h 12% · 37/100 req`) and today's requests against the daily limit. The
  window share turns to the warning colour at 80% and the error colour at 95%.
- When the gateway does not send the 5-hour window, the bar shows the plan and today's
  requests, and the popover shows today's requests (or tokens) only.
- Signed out, the right side reads `Signed out: /login`.

## 7.0.0-alpha.3 (2026-11-02)

The third Neuron alpha. The full-screen UI now shows what a turn is doing while it
happens: each tool call as the model writes it, output tokens as they arrive, the task
list, sub-agents, the files the session changed, diffs, commits and rewinds to a
checkpoint. It asks before working in a new folder and signs you in on first launch.
Scripts can continue a saved session, and a denied tool call names the allow rule that
lets it run. Weaker and auto-routed models get more help staying on task.

It is still a GitHub prerelease. The [known limitations of 7.0.0-alpha.1](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/v7.0.0-alpha.1/CHANGELOG.md#known-limitations)
still apply, except the ones about npm and PyPI and the ones this release fixes below.

### npm and PyPI

- 7.0.0-alpha.2 never reached npm: npm 11 read the tarball path `dist/<name>.tgz` as a
  GitHub repository and the publish failed before it reached the registry. The release
  workflow now publishes `./dist/<name>.tgz`, and a packaging test guards the path.
- The npm and PyPI packages are published after this GitHub release, once its assets
  have been checked, so they can appear later than the release itself.
- On npm this alpha is published under the default `latest` dist-tag. When
  `npm view @zero-x.live/neuron version` prints `7.0.0-alpha.3`,
  `npm install --global @zero-x.live/neuron` installs it; while it still prints 6.2.0,
  use the install scripts.
- On PyPI the alpha is a pre-release (`7.0.0a3`), installed with
  `pip install --pre neuroncli` once `pip index versions neuroncli --pre` lists it. No
  Neuron 7 build has reached PyPI yet; until one does, pip installs the unsupported
  6.2.4, so use the install scripts or npm.
- The old 6.2.x builds (npm 6.2.0, PyPI 6.2.4 and earlier) remain unsupported.

### Full-screen UI: live turn

- Every tool call gets its row from the first token the model writes for it. A file
  call reads Write or Edit with the path, lines and size so far and its last lines
  dimmed; a command shows as soon as it is written, then Running... until its result;
  a sub-agent call gets a row with its description, type, progress and the start of its
  answer. A call the model never finished writing ends as failed ("The response ended
  before this call was complete"), and a retried request drops the rows of the attempt
  that failed.
- The status line replaces "thinking..." with a spinner and what the turn is doing:
  Thinking, Responding, Writing or Editing a file, Running a command, Waiting for
  approval, Waiting for the model. A stream silent for more than 10 seconds says how
  long it has had no tokens. The right side shows the elapsed time, the turn's output
  tokens so far and "esc to interrupt". The footer's token counts include the running
  turn, marked `~`, until its usage arrives.
- While the agent keeps a task list, a todo strip above the input shows progress and
  the item in progress (`Todos 2/5`); `/todo` shows the whole list. A cut-off or failed
  TodoWrite keeps the previous list.
- The footer shows the model `auto` picked (`auto → <model>`).

### Full-screen UI: changes, diffs, commits and rewind

- The engine keeps a checkpoint for every prompt, with the files the agent and its
  sub-agents changed and the shell commands they ran.
- A line above the input counts the session's changed files and lines
  (`3 files changed +120 -4 · /changes`). `/changes` lists them; Enter shows a file's
  diff, `a` shows all of them.
- `/diff [path]` shows the working tree against HEAD (outside git, the agent's changes
  this session).
- `/commit` drafts a commit message with the session's model and shows a commit card
  (branch, files, message); Enter commits, `e` edits, Esc cancels. `/commit <message>`
  commits with that message. Files that can hold credentials are never sent to the model and are
  left out of `git add -A`.
- `/rewind` picks a checkpoint, then code and chat, chat only or code only, and shows a
  preview of what is restored, removed or cannot be restored (shell commands are not
  undone) before Enter rewinds.
- The TodoWrite list is kept in Neuron's home folder instead of `.clawd-todos.json` in
  the working tree, so it no longer shows in `git status` or ends up in a commit.

### Full-screen UI: trust, sign-in and reports

- A folder Neuron has not been trusted in opens on "Before Neuron works here": the
  folder, its git root, what Neuron may do there and what the project's settings would
  turn on (nothing starts while you decide), then Allow or Quit. The line REPL asks the
  same question in the same words, as a two-choice menu.
- With no Zero-X login, the first screen signs you in, then asks about the folder, then
  opens the chat. Esc leaves sign-in for later. If Zero-X cannot be reached, the
  sign-in panel says so and how to retry.
- `/usage`, `/init`, `/memory` and `/doctor` open their report in a panel above the
  input.
- `/new`, `/clear` and the history screen's New chat start a fresh conversation in the
  engine too; a chat picked in `/history` resumes its session. Agent modes the engine
  does not support are no longer offered, `/image` is hidden while images do not reach
  the model, and `/upgrade` opens the pricing page.

### Agent loop for weaker models

- Tool arguments that are almost JSON (code fences, a JSON string, trailing commas,
  raw newlines, a missing closing brace) are repaired, and the model is told so. An
  unknown tool name gets "did you mean" suggestions.
- `edit_file` accepts a unique match that differs only in trailing whitespace or
  indentation, and says which match it used. `write_file` and `edit_file` refuse
  placeholders such as `// ... existing code ...` and cut-off rewrites that would
  drop code.
- After a turn that changed files, the model is asked once to check its changes against
  the request, with the results of the project's checks (`cargo check`, the typecheck
  script, `go vet`, `py_compile`, or `.neuron/checks.json`). The checks run by
  themselves only when your permission settings allow them without asking.
  `NEURON_COMPLETION_CHECK=0` and `NEURON_AUTO_CHECKS=0` turn this off.
- `NEURON_REVIEW=1` adds a read-only reviewer sub-agent to that check
  (`NEURON_REVIEW_MODEL` picks its model). An experience library
  (`NEURON_EXPERIENCE=0` turns it off) adds notes to a model's system prompt about the
  mistakes it keeps making.
- A turn that ends on an announcement ("I'll now update the file.") is nudged once to
  make the call. When `auto` routes to a model with a smaller context window, the
  context budget shrinks so compaction happens before the request is rejected.

### Scripts and the line REPL

- `neuron --resume SESSION prompt TEXT` (or `-p TEXT`) continues a saved session from
  a script. `--output-format json` reports `session_id`, `served_model` and
  `completion_checked`.
- Two runs on one session no longer lose turns: a session in use by another run is
  refused ("session X is in use by another neuron run"); read-only commands such as
  `/status` still work on it.
- Piped input meant for the REPL (`printf '/model\n/exit' | neuron`, also behind a
  UTF-8 byte order mark) is refused instead of being sent to the model as one prompt.
  A bare `neuron` without a terminal stops with "nothing to do" instead of asking a
  question nobody can answer. `--resume latest` skips empty sessions. Git Bash's path
  rewriting of `neuron /model` is undone.
- A tool call denied in a non-interactive run names the `permissions.allow` rule that
  lets it run (in user settings), and names Full access only as a last resort. The
  named rules are checked to actually allow the call. `NEURON_DENIAL_LIMIT` raises the
  number of denials after which a run stops (default 5).
- `/model` in the line REPL lists models with their live status: usable, temporarily
  unavailable (and until when) or not available. Choosing an unavailable model, or
  passing one to `neuron prompt --model`, warns; an out-of-capacity error names models
  that are usable now.
- The retry of a broken stream toward Zero-X waits 2 to 5 seconds and has a 100-second
  first-token deadline, and text from the failed attempt is marked
  "(partial reply discarded - retrying)".
- `neuron --help` lists only what runs: every command on its "Resume-safe commands"
  line runs under `neuron --resume`, and every option the parser takes is listed.
  `--resume X /context clear` and `/skills install` under `--resume` behave as in a
  session. README, USAGE and `neuron-tui --help` match what ships.

## 7.0.0-alpha.2 (2026-10-26)

The second Neuron alpha, and the first one on npm and PyPI: both packages now install
the new Neuron. It is still a GitHub prerelease. It says clearly when the model
declines a request, keeps the model's reasoning apart from its answer, never runs a
tool call from a reply that was refused or cut off, and lets long answers finish
streaming.

The [known limitations of 7.0.0-alpha.1](https://github.com/RAHUL-DevelopeRR/neuron-v2/blob/v7.0.0-alpha.1/CHANGELOG.md#known-limitations) still apply, except the ones about npm and PyPI.

### npm and PyPI

- `npm install --global @zero-x.live/neuron` installs this alpha: it is npm's default
  `latest` version, so `@next` is no longer needed. Run the same command to upgrade.
- On PyPI the alpha is a pre-release, so pip needs `--pre`:
  `pip install --pre neuroncli` (`pip install --upgrade --pre neuroncli` to upgrade).
- The old 6.2.x builds (npm 6.2.0, PyPI 6.2.4 and earlier) were built outside the
  release pipeline and are unsupported. Upgrade with the commands above.
- The GitHub release is still a prerelease, so `install.sh` and `install.ps1` need its
  version (`--version 7.0.0-alpha.2`, `-Version 7.0.0-alpha.2`); the README has the
  full commands.

### Refusals and reasoning

- When the model declines to continue, Neuron says so instead of failing or showing an
  empty reply, and keeps whatever the model wrote before. The line REPL ends the turn
  with "The model declined to continue this request." and the provider's reason, in
  Neuron's words and without provider or model names. The full-screen UI ends the
  message with a "Declined" banner of its own, below the answer. `--output-format json`
  reports `outcome` `refused` and a `refusal` object (`message`, `note`,
  `tool_calls_not_run`); with `--compact`, the refusal line goes to standard error.
- The model's reasoning stays apart from its answer in the line REPL, the full-screen
  UI and headless output, and is never part of the JSON `message`. A thinking block no
  longer starts with blank lines.

### Tool calls and cut-off replies

- No tool call from a refused reply, or from one cut off at the output token limit,
  runs, even when its arguments are complete. Each gets an error result saying why,
  and the full-screen UI shows such calls as not run. Sub-agents follow the same rule,
  and a sub-agent whose stream breaks off runs none of that reply's calls: it tries a
  fallback model or fails with a retryable error.
- Tool arguments that are not a JSON object, such as arguments cut off mid-stream, go
  back to the model as a tool error instead of failing the turn. They are checked
  before hooks, permissions or the tool run.
- A reply cut off at the output token limit while the model was still reasoning, before
  any answer, no longer fails the turn with "assistant stream produced no content". The
  turn continues once, telling the model that its reasoning used the whole budget; cut
  off again, it ends as `output_truncated`. Neuron now counts the tokens of such a
  reply, also in a restored session.

### Streams

- A streamed answer has no total time limit, so long answers are no longer cut off at
  5 minutes. The connection must open within 30 seconds, and a stream that receives
  nothing for 120 seconds (keep-alive messages count) ends with a retryable error that
  names the model.
- Requests answered in one piece, such as the non-streaming fallbacks, keep their
  5-minute total limit and have no idle limit, since their reply arrives only once it
  is complete.

### Packaging

- Removed a stray submodule entry (`opencode`) that had no `.gitmodules` URL. It broke
  the checkout in the release workflow's npm and PyPI jobs, which is why 7.0.0-alpha.1
  never reached npm (PyPI uploads were also paused then). A packaging test now fails on
  any such entry.
- A manual run of the release workflow chooses the npm dist-tag (`npm_dist_tag`:
  `next`, the default, or `latest`). A tag push still publishes a prerelease under
  `next` and a release under `latest`.

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
