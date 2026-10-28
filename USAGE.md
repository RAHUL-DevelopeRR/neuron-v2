# Using Neuron

This guide covers day-to-day use of the `neuron` command. Installation is in the
[README](README.md#install); `neuron --help` lists every flag of the version you have.

## Start a session

```sh
cd your-project
neuron
```

On a terminal, `neuron` opens the full-screen UI, `neuron-tui`, which the installers
and launchers put beside `neuron`. Type a request, or a slash command such as `/help`.
Approve or deny tool calls in its dialog (Esc denies; Ctrl+C denies and interrupts the
turn), sign in with `/login` and pick a model with `/model`. Ctrl+C interrupts the
current turn, and `/exit` or Ctrl+C twice quits. `neuron --continue` reopens the
latest chat for this folder, and `neuron --continue CHAT` a given one. Shell commands
typed in the UI (`!cmd`, `/bash`) are not available yet.

The UI does not ask whether you trust the folder: until you do, the folder's own
settings, hooks and MCP servers are not loaded, and `/status` says how to trust it
(see [Workspace trust](#workspace-trust)).

`neuron --no-tui` (or `NEURON_NO_TUI=1`) keeps the line REPL, which also runs when the
UI is missing. The first time you run it in a directory it asks whether you trust it;
Neuron reads, writes and runs commands only after you agree. Press Ctrl+C to interrupt
the current turn and `/exit` to leave.

For one-off and scripted use:

```sh
neuron prompt "summarize the open TODOs in src/"     # one prompt, then exit
neuron "summarize Cargo.toml"                        # shorthand for the same
neuron --output-format json prompt "list the public functions in lib.rs"
neuron --compact "summarize Cargo.toml" | wc -l      # final answer only, for pipes
git diff | neuron prompt "review this change"        # piped input goes with the prompt
```

Piped standard input is sent to the model with the prompt, up to 128 KiB; with a prompt
on the command line, a pipe that sends nothing for 500 ms is not waited for. In a
script or CI, `neuron prompt` asks nothing: see [Permission modes](#permission-modes)
for what happens to tool calls that need approval, and why such a run can exit with
code 2.

Piped text without a prompt on the command line (`echo "fix the bug" | neuron`) is the
prompt, sent to the model as one message. So slash commands can't be piped:
`printf '/status\n/exit' | neuron` is refused, as is piped input with `--no-tui`. Use the
subcommands (`neuron status`) or `neuron --resume latest /status` instead. A bare `neuron`
with no terminal and nothing piped (`neuron </dev/null`) stops with "nothing to do"
rather than starting a session.

`--output-format json` prints one JSON object when the turn ends. `message` is the
final answer text (never the model's reasoning), `outcome` says how the turn ended
(`completed`, `output_truncated`, `loop_detected`, `cancelled` or `refused`) and
`stop_reason` is the provider's last stop reason. When the model declines to continue
(`outcome` `refused`, `stop_reason` `refusal`; OpenAI-compatible providers call it
`content_filter`), `message` keeps whatever it wrote before, and `refusal` holds the
line Neuron shows (`message`), the provider's explanation with provider and model
names left out (`note`, or `null`) and how many tool calls of that response were not
run (`tool_calls_not_run`); otherwise
`refusal` is `null`. With `--compact`, standard output keeps the answer only and the
refusal line goes to standard error. Tool calls from a refused response, or from one
cut off at the output token limit, are never run.

## Sign in

Neuron uses your Zero-X account unless you choose another provider.

| Command | What it does |
| --- | --- |
| `neuron login` (or `neuron auth login`) | Prints a link, <https://neuron.zero-x.live/device>, and a short code such as `BDWP-HQPK`, and opens the link in your browser when this machine has one. Approve the code there while signed in to Zero-X (on this machine or any other device, such as your phone), and Neuron continues on its own. Nothing listens on a local port, so this works over SSH, in WSL and in containers; `--no-browser` (or `NEURON_NO_BROWSER=1`) only skips opening the browser. The code expires after 10 minutes. The session is stored in the OS credential store on macOS and Windows, otherwise in `~/.neuroncli/credentials.json` (owner-only); see [SECURITY.md](SECURITY.md#how-neuron-handles-credentials). |
| `neuron login --with-token` | Reads a credential from standard input instead, for CI. Use a personal API key (`nzx_…`) from the Zero-X dashboard, or a session token (`ses_…`). |
| `neuron auth status` | Checks the credential with the gateway and prints where it came from, the plan, the expiry and today's usage. |
| `neuron logout` (or `neuron auth logout`) | Revokes a session on the gateway and deletes the local copy (API keys are revoked in the dashboard). |

Inside a session, `/login` and `/logout` run the same flows.

Only approve a code you started yourself: the approval page shows which device asked
and where from, and warns when that differs from your browser's location. Zero-X never
asks you to enter a code sent by email, chat or phone.

```sh
# CI: keep the key in your CI secret store, never in the repository
printf '%s' "$ZEROX_API_KEY" | neuron auth login --with-token
```

You can also set `NEURON_TOKEN` to a key or session for a single process; it takes
precedence over the stored login and over any provider choice, and is not saved.
Neuron removes it (and any other `ses_…` or `nzx_…` value) from the environment of the
commands, plugin tools and MCP servers it starts; hooks run with Neuron's own
environment and do see it. `neuron auth logout` does not clear it, so
unset it yourself. `NEURON_API_BASE` changes the gateway URL (default
`https://zero-x.live/v1`; plain HTTP is accepted only for a loopback address).

Sessions expire after 30 days by default. If the gateway answers `invalid_session`,
the credential expired or was revoked: run `neuron auth login` again. Revoking all
sessions from the Zero-X dashboard signs out every machine.

## Plans, usage and limits

| Command | What it does |
| --- | --- |
| `neuron usage` | Prints your plan, today's requests and tokens with the reset time, and your credit balance. |
| `/usage` | The same, inside a session. |
| `/upgrade` | Opens <https://zero-x.live/pricing>. |
| `/cost` | Token usage of the current session. |

Neuron reports gateway limits with the reason and what to do next:

| Error | Meaning | What Neuron does |
| --- | --- | --- |
| `insufficient_quota` | Daily request or token cap reached | Stops and shows the reset time and the upgrade link. No retry. |
| `rate_limited` | Too many requests per minute, or too many at once | Waits for the `Retry-After` time and retries, at most twice; each wait may be up to 90 seconds and all of them up to 180 seconds, shown as a countdown. |
| `capacity_exhausted` | The shared free pool for this model is used up | Same as `rate_limited`. A longer wait than that stops with the error. |
| `upstream_error`, `upstream_timeout`, `service_unavailable` | The upstream provider failed or stalled, or the gateway is briefly unavailable; the gateway refunds most of these requests | Retries at most twice, waiting at most 60 seconds in total. |
| `model_not_found` | The model id is not in the catalog | Stops; pick one from `/model`. |
| `upgrade_required` | The model is not in your plan | Stops and shows the upgrade link. |
| `insufficient_credits` | Paid capacity needed, credit balance too low | Stops and shows the upgrade link. |
| `region_unavailable` | The free pool is not offered in your region | Stops; choose another model. |
| `context_length_exceeded` | No available model fits the request, including a prompt above a free model's per-minute token budget | Stops with the gateway's message and, when a paid pool would take the request, the upgrade link; shorten the prompt or `/compact` the session. |
| `invalid_session` | The credential expired or was revoked | Stops and asks you to run `neuron auth login`. |

Neuron waits up to 100 seconds for a gateway response to start, also after tool
results, so the gateway has time to try other upstream providers first. A provider you
call directly ([Bring your own key](#bring-your-own-key)) gets 90 seconds, and 60 after
tool results. `NEURON_FIRST_TOKEN_TIMEOUT_SECS` sets both of these first-token
timeouts, and waits between retries do not count toward them. A response that does not
start in time is sent once more after 1 to 2 seconds, and one that breaks off after 2
to 5 seconds, only while the retry limit above still allows a request, so one model
call sends at most three requests to the gateway. When part of a reply that broke off
was already printed, a `(partial reply discarded — retrying)` line follows it before
the reply starts again; `--output-format json` and `--compact` print only the final
answer, so they need no such line.

The free plan's limits and the providers behind it are described in the
[README](README.md#plans-and-limits).

## Models

```sh
neuron --model auto                 # let the gateway choose (the default)
neuron --model gemini/gemini-2.5-flash
```

Inside a session, `/model` shows the active model and the catalog, and
`/model <id>` switches. The list comes from the gateway (`GET /v1/models`, cached for
10 minutes in `~/.neuroncli/catalog.json`) and marks models that are unavailable to
you with the reason: not in your plan, not offered in your region, or not configured
on the gateway. `auto` picks a tool-capable model whose context window fits the
request. `NEURON_MODEL` sets the model for a process.

Free pools have small per-minute token budgets, so the first request of a session is
kept small: the system prompt lists the bundled skills by name and the model loads a
skill's instructions only when it needs them, and MCP server tools are offered
through `ToolSearch` instead of with every request. In a one-file repository the first
request is about 17,000 characters (roughly 4,500 tokens).

## Permission modes

| Mode | Neuron may, without asking | Needs your approval |
| --- | --- | --- |
| `read-only` | Read and search files in the workspace, run read-only shell commands | Nothing else is allowed |
| `workspace-write` (default) | The above, plus edit files in the workspace | Other shell commands, searches or writes outside the workspace, writes to `.git` or Neuron settings, fetches from a new domain, sub-agents, and MCP and other tools that need full access |
| `danger-full-access` | Everything | Nothing |

A shell command (bash or PowerShell) counts as read-only only when every command in
the line is a known read-only program with safe arguments, writes nowhere except
`/dev/null` and names no path outside the workspace; pipes, `&&`, redirections and
substitutions are all checked. When Neuron asks, it shows the tool, the mode it needs,
the reason and the input, and runs the call only if you answer `y`; when standard
output is redirected, the question goes to standard error. When standard output and
standard error are both redirected, nobody would see the question, so the call is
denied instead.

`neuron prompt` asks only when standard input is a terminal. In a script or CI
(standard input a pipe, a file or `/dev/null`) a call that needs approval is denied
without asking, the model is told that nothing ran, and questions from the model are
refused so it decides itself. After 5 denied calls the turn stops. A run that denied a
call ends with a warning naming the denied tools and exits with code 2. Each denial
names the `permissions.allow` rule that would let the call run (`"bash(npm:*)"` for
`npm test`, `"WebSearch"` for a web search); put it in your user settings
(`~/.neuron/settings.json`), because a script never gets the trust question and an
untrusted workspace's own allow rules are ignored. Full access
(`--permission-mode danger-full-access`) is named only as a last resort, for a
disposable environment.

`permissions` rules in settings can allow or deny tools and command prefixes in
advance, for example `{"permissions": {"allow": ["bash(git:*)"]}}`.

Choose the mode with `--permission-mode MODE`, switch inside a session with
`/permissions MODE`, or set a default in settings:

```json
{ "permissionMode": "read-only" }
```

`--dangerously-skip-permissions` turns every check off for that run; use it only in a
disposable environment such as a container. `--allowedTools read,glob` limits which
tools exist at all, whatever the mode. Settings can also carry `permissions` allow, ask
and deny rules for individual tools.

## Sessions

Sessions are saved to `.neuron/sessions/<workspace-id>/<session-id>.jsonl` in the
project. Every `neuron prompt` run starts a new session.

```sh
neuron --resume latest                         # continue the newest session
neuron --resume latest --permission-mode read-only   # ...with different launch flags
neuron --resume latest /status /diff           # run resume-safe commands without the REPL
neuron --resume latest prompt "now add tests"  # send it another prompt, then exit
neuron export conversation.md                  # write the latest session as Markdown
```

`latest` is the newest session with messages: a session left before its first prompt is
skipped. On a terminal, `--resume` without slash commands continues the session
interactively.
`--resume SESSION prompt "..."` (or `-p "..."`) works anywhere, scripts included: it runs
one turn as `neuron prompt` does, with the session's history, and saves the turn to the
same file. With `--output-format json` its stdout is the turn's JSON answer, as for
`neuron prompt`. `--model`, `--permission-mode`, `--allowedTools`, `--reasoning-effort`
and the other launch flags apply to it whether you put them before or after `--resume`;
without `--model` it continues on the model the session used.

Inside a session: `/session list`, `/session switch <id>`, `/resume <path>`,
`/compact` to shrink history, `/clear` to start over.

## Common slash commands

| Command | Purpose |
| --- | --- |
| `/help` | All commands |
| `/status` | Model, permission mode, workspace and session |
| `/model [id]` | Show or switch the model |
| `/permissions [mode]` | Show or switch the permission mode |
| `/usage`, `/upgrade` | Account usage; open the pricing page |
| `/login`, `/logout` | Sign in or out of Zero-X |
| `/cost` | Token usage of this session |
| `/compact`, `/clear` | Shrink or reset the conversation |
| `/diff` | Changes in the working tree |
| `/init` | Create a starter `NEURON.md` for the project |
| `/mcp`, `/skills`, `/plugins` | Inspect and manage integrations |
| `/doctor` | Diagnose auth, configuration and sandbox setup |
| `/exit` | Leave |

`neuron status`, `neuron doctor` and `neuron sandbox` print the same reports without
starting a session.

## Bring your own key

Set the key for the provider you want to use:

| Provider | Variables |
| --- | --- |
| OpenAI or any OpenAI-compatible endpoint | `OPENAI_API_KEY`, optional `OPENAI_BASE_URL` |
| Local OpenAI-compatible server | `OPENAI_BASE_URL=http://127.0.0.1:<port>/v1` (no key needed) |
| Anthropic | `ANTHROPIC_API_KEY` (or `ANTHROPIC_AUTH_TOKEN`), with `auto` or a `claude-*` model |
| xAI | `XAI_API_KEY`, with a `grok-*` model |
| OpenRouter | `OPENROUTER_API_KEY` (OpenRouter is only available with your own key) |
| Azure AI Foundry | `AZURE_OPENAI_API_KEY`, `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_MODEL` |

Neuron picks the provider in this order and names it, with the credential source, on
startup:

1. `NEURON_TOKEN` in the environment: the Zero-X gateway.
2. An explicit choice: `--provider NAME`, then `NEURON_PROVIDER`, then the `provider`
   setting. Names: `zero-x`, `openai`, `openrouter`, `azure`, `anthropic`, `xai`,
   `local`. A misspelled name is an error that names the file or flag.
3. Your stored Zero-X login.
4. Keys in the environment, in this order: Anthropic (for `auto` or `claude-*`
   models), xAI (for `grok-*` models), Azure (while its daily quota lasts and the
   endpoint answers), OpenAI, a loopback `OPENAI_BASE_URL`, OpenRouter.
5. Otherwise Neuron asks you to run `neuron auth login`.

So while you are signed in to Zero-X, keys in the environment are ignored until you
select their provider. Requests then go straight to that provider under your account
and its terms; Zero-X limits and credits do not apply. The key stays in memory and is
not exported to the commands Neuron runs, and Neuron removes provider keys and any
variable named `*_API_KEY`, `*_TOKEN` or `*_SECRET` from the environment of the
commands, plugin tools and MCP servers it starts, including keys you exported yourself
(a value set in an MCP server's `env` still applies). Hooks are the exception: they run
with Neuron's own environment, including any keys you exported.

## Configuration files

| File | Scope |
| --- | --- |
| `~/.neuron/settings.json` | Your defaults (the directory moves with `NEURON_CONFIG_HOME`) |
| `.neuron/settings.json` | Project settings, safe to commit |
| `.neuron/settings.local.json` | Your overrides for this project; keep it out of version control |

Later files override earlier ones. `/config` shows the merged result. Settings are
checked when they load: an unknown key or a value of the wrong type is an error that
names the file, the line and the closest known key. Commonly used keys:

| Key | Purpose |
| --- | --- |
| `model` | Default model |
| `permissionMode` | Default permission mode |
| `permissions` | `allow`, `ask` and `deny` rules for individual tools |
| `provider` | Provider to use (see [Bring your own key](#bring-your-own-key)) |
| `maxIterations` | Model calls allowed in one turn (default 50); `NEURON_MAX_ITERATIONS` overrides it |
| `hooks` | Commands run before and after tool calls |
| `mcpServers`, `defaultMcpServers`, `disabledMcpServers` | MCP servers |
| `defaultSkills` | `false` leaves the bundled skills out of the system prompt |

Default skills and MCP servers are described in
[default integrations](docs/DEFAULT_INTEGRATIONS.md).

### Workspace trust

A cloned repository's own settings files (`.neuron.json`, `.neuron/settings*.json`
and the `.claw` equivalents) can run commands, so Neuron applies them only once you
trust the directory. The first time you start Neuron in a directory it asks; your
answer is saved in `~/.neuroncli/trusted_dirs.txt` (under `NEURON_HOME` if set).
A directory is also trusted when it sits under a `trustedRoots` entry in **your user
settings**; a repository cannot list itself. For headless runs, set
`NEURON_TRUST_WORKSPACE=1` only for checkouts you trust.

Until a directory is trusted, its settings keep only what restricts Neuron:
`permissions.deny` (added to your own deny rules), a read-only `defaultMode` or
`permissionMode`, and `"defaultMcpServers": false` or `"defaultSkills": false`.
Hooks, MCP servers, allow and ask rules, model, aliases, provider and the other keys
are ignored, with a warning naming them. Even in a trusted directory, settings files
cannot set a full-access permission mode; pass `--permission-mode danger-full-access`
or put it in your user settings.

## Environment variables

| Variable | Used by | Purpose |
| --- | --- | --- |
| `NEURON_TOKEN` | neuron | Gateway credential for this process |
| `NEURON_API_BASE` | neuron | Gateway URL (default `https://zero-x.live/v1`) |
| `NEURON_MODEL` | neuron | Model for this process |
| `NEURON_PROVIDER` | neuron | Provider for this process (see [Bring your own key](#bring-your-own-key)) |
| `NEURON_MAX_ITERATIONS` | neuron | Model calls allowed in one turn (default 50) |
| `NEURON_FIRST_TOKEN_TIMEOUT_SECS` | neuron | Seconds to wait for a response to start, also after tool results (default 100 for the gateway; 90, and 60 after tool results, for a provider called directly) |
| `NEURON_CONFIG_HOME` | neuron | Settings directory (default `~/.neuron`) |
| `NEURON_HOME` | neuron | Directory for the login file and caches (default `~/.neuroncli`); setting it also keeps the login out of the OS credential store |
| `NEURON_CREDENTIAL_STORE` | neuron | `file` or `keychain`: force where the login is kept |
| `NEURON_TRUST_WORKSPACE=1` | neuron | Trust the current directory in a headless run (only for checkouts you trust) |
| `NEURON_NO_TUI=1` | neuron | Use the line REPL instead of the full-screen UI |
| `NEURON_TUI_ASCII` | neuron-tui | `1` draws with the classic-console character set everywhere, `0` with the Unicode one |
| `NEURON_VERSION` | install.sh, install.ps1 | Release to install (default: the latest release; a prerelease such as `7.0.0-alpha.2` must be named) |
| `NEURON_INSTALL_DIR` | install.sh, install.ps1 | Install directory |
| `NEURON_BINARY_PATH` | npm and pip launchers | Run this binary instead of the downloaded one |
| `NEURON_CACHE_DIR` | npm and pip launchers | Where downloaded binaries are cached |
| `NEURON_SKIP_DOWNLOAD=1` | npm postinstall | Defer the binary download to the first run |
| `NEURON_DOWNLOAD_BASE` | installers and launchers | Release mirror (HTTPS, or loopback HTTP for tests) |

## Update and uninstall

| Installed with | Update | Uninstall |
| --- | --- | --- |
| `install.sh` | Run it again with the new version: that version's release URL and `--version` (or `NEURON_VERSION`). Without a version it installs the latest stable release, and there is none yet | `rm ~/.local/bin/neuron ~/.local/bin/neuron-tui` |
| `install.ps1` | Run it again with the new version: that version's release URL and `-Version` (or `NEURON_VERSION`). Without a version it installs the latest stable release, and there is none yet | Delete `%LOCALAPPDATA%\Programs\Neuron\bin` and remove it from your user `PATH` if you added it |
| npm | `npm install --global @zero-x.live/neuron` (the alpha is npm's `latest` version) | `npm uninstall --global @zero-x.live/neuron` |
| pip | `pip install --upgrade --pre neuroncli` (the alpha is a pre-release on PyPI, so pip needs `--pre`) | `pipx uninstall neuroncli` or `pip uninstall neuroncli` |

Neuron keeps your login in the OS credential store or `~/.neuroncli/`, and your
settings in `~/.neuron/`; the launchers cache binaries in `~/.cache/neuroncli` (Linux), `~/Library/Caches/neuroncli`
(macOS) or `%LOCALAPPDATA%\neuroncli` (Windows). Run `neuron auth logout` before
deleting them so the session is revoked on the gateway and removed from the
credential store too.

## Troubleshooting

- **The wrong `neuron` runs.** Older packages (npm 6.2.0, PyPI 6.2.4 and earlier) can
  leave a `neuron` earlier on your `PATH`. `command -v neuron` (or `Get-Command neuron`
  on Windows) shows which one wins; the installers warn about this. Remove the old
  package with `pip uninstall neuroncli` or `npm uninstall --global @zero-x.live/neuron`.
- **The first run fails offline.** The npm and pip launchers need GitHub Releases once
  per version. Behind a proxy set `HTTPS_PROXY` (Node 24 also needs
  `NODE_USE_ENV_PROXY=1`), or download the binary yourself and set
  `NEURON_BINARY_PATH`.
- **Checksum mismatch.** Nothing was installed. Retry; if it persists, report it as
  described in [SECURITY.md](SECURITY.md).
- **Windows SmartScreen or macOS Gatekeeper warnings.** The binaries are not
  code-signed yet. Files fetched by the installers are not quarantined on macOS; if
  you downloaded a binary with a browser, check its SHA-256 and then run
  `xattr -d com.apple.quarantine ./neuron-macos-arm64`.
- **Git Bash runs `neuron /model` as a prompt.** Git Bash (MSYS) rewrites an argument
  that starts with `/` into a Windows path, so `/model` arrives as
  `C:/Program Files/Git/model` (`C:/msys64/model` under MSYS2). Where a slash command
  goes (the first argument, or any argument after `--resume`), a path like that which
  names a slash command is treated as the slash command, and Neuron says so on stderr
  (not with `--output-format json`). Elsewhere it stays a path, so the safe choices are
  the subcommands (`neuron skills`, `neuron status`), or `MSYS_NO_PATHCONV=1` for the
  command: `MSYS_NO_PATHCONV=1 neuron --resume latest /status`.
- **A script warns "this workspace is not trusted".** Neuron never asks the trust
  question without a terminal: a script runs, but the folder's own settings (hooks,
  MCP servers, model, allow rules) are ignored, with that warning naming them. Set
  `NEURON_TRUST_WORKSPACE=1` for that run (only for checkouts you trust), add the
  folder to `trustedRoots` in `~/.neuron/settings.json`, or run `neuron` there once in a
  terminal and press 1.
- **Anything else.** `neuron doctor` checks auth, configuration and the workspace.
  Include its output, `neuron --version` and your install method when you open an
  issue.
