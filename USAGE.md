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
latest chat for this folder, and `neuron --continue CHAT` a given one. `neuron "fix the
failing test"` opens the UI and sends that prompt first, and `neuron --resume SESSION`
opens it on a saved session, showing its conversation. Shell commands typed in the UI
(`!cmd`, `/bash`) are not available yet.

When the agent asks you something (its `AskUserQuestion` tool), the question appears
where it was asked. If it offers choices, a form takes the input's place: ↑/↓ and
Enter pick one, or write your own answer in its last field; Esc closes the form and
the next thing you type answers instead. A question without choices is answered by
typing in the input.

The first time you open the UI in a folder it shows "Before Neuron works here" and
asks you to allow Neuron in the folder or quit; until you allow it, the folder's own
settings, hooks and MCP servers are not loaded (see [Workspace trust](#workspace-trust)).

The status bar under the input shows the model, the thinking level, the folder and
branch, the permission mode and the context meter. Click a part, or press Alt+M, Alt+T,
Alt+W, Alt+P or Alt+C, to open its picker (on macOS, Option+M and so on once the
terminal uses Option as Meta). Shift+Tab cycles the permission mode. `/effort <level>`
sets the thinking level for the next turn and `/effort default` goes back to the
model's own; `/branch` lists the local branches, `/branch <name>` switches and
`/branch new <name>` creates one and switches to it.

The UI needs a terminal of at least 60 columns by 16 rows. In a smaller one `neuron`
starts the line REPL instead and says why (`neuron "prompt"` answers it as plain text,
and `neuron --continue` asks you to make the window bigger); a terminal made smaller
while the UI runs shows "Terminal too small" until it is big enough again (the chat
and a running turn carry on underneath; keys are ignored until the chat is back, but
Ctrl+C still stops a turn). In a terminal set up for Chinese, Japanese or Korean, where the
status bar's `│ ● ○` glyphs take two columns each, the bar measures them that way
(from `LC_ALL`, `LC_CTYPE` or `LANG`; `NEURON_AMBIGUOUS_WIDE=1` or `0` says so
explicitly).

`neuron --no-tui` (or `NEURON_NO_TUI=1`) keeps the line REPL, which also runs when the
UI is missing. The first time you run it in a directory it asks whether you trust it;
Neuron reads, writes and runs commands only after you agree. Press Ctrl+C to interrupt
the current turn and `/exit` to leave.

For one-off and scripted use:

```sh
neuron prompt "summarize the open TODOs in src/"     # one prompt, then exit
neuron -p "summarize Cargo.toml"                     # shorthand for the same
neuron --output-format json prompt "list the public functions in lib.rs"
neuron --compact "summarize Cargo.toml" | wc -l      # final answer only, for pipes
git diff | neuron prompt "review this change"        # piped input goes with the prompt
```

`neuron "..."` without `-p` is the same one prompt when standard input or output is not
a terminal; on a terminal it opens the full-screen UI. On a terminal, `-p` and
`neuron prompt` start with one line on standard error naming the model, the permission
mode and the plan.

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
(`completed`, `output_truncated`, `loop_detected`, `no_progress`, `budget_stopped`,
`cancelled` or `refused`) and
`stop_reason` is the provider's last stop reason. When the model declines to continue
(`outcome` `refused`, `stop_reason` `refusal`; OpenAI-compatible providers call it
`content_filter`), `message` keeps whatever it wrote before, and `refusal` holds the
line Neuron shows (`message`), the provider's explanation with provider and model
names left out (`note`, or `null`) and how many tool calls of that response were not
run (`tool_calls_not_run`); otherwise
`refusal` is `null`. `session_id` names the session the turn was saved to, which
`--resume` takes (see [Sessions](#sessions)). With `--compact`, standard output keeps
the answer only and the refusal line goes to standard error. Tool calls from a refused
response, or from one cut off at the output token limit, are never run.

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
| `neuron usage` | Prints your plan, today's requests and tokens with the reset time, your plan's token window when it has one (`Token window     Free 640K/1M, resets 14:20 UTC`, then the exact figures), and your credit balance. |
| `/usage` | The same, inside a session. |
| `/status` | Also ends with a `Plan` section: the token window (`Free 640K/1M, resets 14:20 UTC`), the size of the next request, and the working set. |
| `/upgrade` | Opens <https://zero-x.live/pricing>. |
| `/cost` | Token usage of the current session. |

### The Free token window

Free counts tokens over a rolling 5-hour window (1,000,000 tokens), and that window,
not the daily figure, is what refuses requests. Every step of an agent turn sends the
whole conversation again, so a long task can use the window quickly. On a plan with a
window Neuron:

- shows the window in `/usage`, `neuron usage` and `/status`, and on the right of the
  full-screen UI's status bar (`Free · 5h 640K/1M · resets 14:20 · 37/100 req`, in your
  local time; a narrow terminal keeps `640K/1M`, amber from 80% and red from 95%), keeps
  it current from the quota headers of each reply, and warns once when 90% of it is
  used. Once the
  window's reset time has passed, what is left is shown as unknown (`Free ?/1M`) until
  the account is read again, which Neuron does in the background;
- keeps a smaller working set: about 40,000 tokens of conversation (system prompt and
  tool definitions not counted). Once the conversation reaches half of it, tool results
  older than the last four become short stubs that name the tool and keep the start of
  the output (the model runs the tool again when it needs the rest); near the working
  set, older history is compacted as usual. `workingSetTokens` in the settings (or
  `NEURON_WORKING_SET_TOKENS`) sets the size, also on other plans, and `0` turns it
  off;
- asks before a request would need more tokens than the window has left ("Send
  anyway" or "Stop here"); a stopped turn sends nothing, and `/compact` makes the
  conversation smaller. `neuron prompt` asks too when stdin is a terminal; a run with
  nobody to ask (a script) prints a warning on stderr and sends the request, and the
  gateway decides. Sub-agents of the `Agent` tool keep the same working set and do not
  ask: one whose next request the window does not cover stops and reports that, and
  the conversation's own next request is asked about as above.

On the gateway Neuron also sends a `prompt_cache_key` (a hash of the session id, never
the id) so the requests of one session can share a provider's prompt cache, and with
the `auto` model it marks requests that carry tools so `auto` picks a model that calls
tools well.

Neuron reports gateway limits with the reason and what to do next:

| Error | Meaning | What Neuron does |
| --- | --- | --- |
| `insufficient_quota` | Daily request or token cap reached | Stops and shows the reset time and the upgrade link. No retry. |
| `rate_limited` | Too many requests per minute, or too many at once | Waits for the `Retry-After` time and retries, at most twice; each wait may be up to 90 seconds and all of them up to 180 seconds, shown as a countdown. |
| `capacity_exhausted` | The shared free pool for this model is used up | Same as `rate_limited`. A longer wait than that stops with the error, which names up to three models that have room now (or `auto`). |
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
10 minutes in `~/.neuroncli/catalog.json`) with each model's current status
(`GET /v1/models/status`, asked for again at most every 30 seconds). Models you can use
now come first, with busy or degraded ones marked; models out of capacity until a known
time are listed under "Temporarily unavailable" with when they are back; models you
cannot use are listed under "Not available" with the reason: not in your plan, not
offered in your region, or not configured on the gateway. Choosing one of the last two
kinds with `/model <id>` warns, and so does `neuron prompt` with a model other than
`auto`, which asks for the status once before its request; the request is still sent.
`auto` picks a tool-capable model whose context window fits the request, and routes
around models out of capacity. `NEURON_MODEL` sets the model for a process.

When a request to create or change files is answered in chat with no tool call,
Neuron reminds the model once to use its tools; if the next reply still makes no tool
call, the request runs once more on a model known to call tools. That model is the
first of `toolCapableModels` in your settings or, on `auto`, an available
tool-capable model from the catalog. When a provider stays unavailable after the
client's own retries, the turn moves on to the next model of
`providerFallbacks.fallbacks`. Settings can also give some jobs their own model:

```json
{
  "toolCapableModels": ["gemini/gemini-2.5-pro"],
  "autoToolModel": "gemini/gemini-2.5-pro",
  "subagentModel": "gemini/gemini-2.5-flash",
  "summaryModel": "gemini/gemini-2.5-flash",
  "reviewerModel": "gemini/gemini-2.5-flash",
  "providerFallbacks": { "fallbacks": ["gemini/gemini-2.5-flash"] }
}
```

`autoToolModel` replaces `auto` on requests that carry tools, `subagentModel` is the
`Agent` tool's model when a call names none, `summaryModel` drafts short texts such as
commit messages, and `reviewerModel` runs the completion check. An unset key leaves
that job on the session's model.

Free pools have small per-minute token budgets, so the first request of a session is
kept small: the system prompt lists the bundled skills by name and the model loads a
skill's instructions only when it needs them, and MCP server tools are offered
through `ToolSearch` instead of with every request. In a one-file repository the first
request is about 17,000 characters (roughly 4,500 tokens).

## Permission modes

| Mode | Neuron may, without asking | Needs your approval |
| --- | --- | --- |
| `read-only` | Read and search files in the workspace, run read-only shell commands | Nothing else is allowed |
| `workspace-write` (default) | The above, plus edit files in the workspace and run build, test, run and package-install commands that stay in it (see below) | Other shell commands, searches or writes outside the workspace, writes to `.git` or Neuron settings, fetches from a new domain, sub-agents, and MCP and other tools that need full access |
| `danger-full-access` | Everything | Nothing |

A shell command (bash or PowerShell) counts as read-only only when every command in
the line is a known read-only program with safe arguments, writes nowhere except
`/dev/null` and names no path outside the workspace; pipes, `&&`, redirections and
substitutions are all checked.

In `workspace-write` mode these commands also run without asking (the auto-allow list):
`npm`, `pnpm`, `yarn` and `bun` installs, scripts and tests (`npm install`, `npm ci`,
`npm run dev`, `pnpm build`, `yarn add react`), `npx`/`bunx`, `node`, `tsc`, `cargo`
builds, tests and runs (`cargo build`, `cargo test`, `cargo add`), `python` and
`python -m ...`, `pip`/`uv` installs, `pytest`, `go build`/`test`/`run`/`mod`, `make`,
and `deno run`/`task`/`test`. Each part of a line (`&&`, `||`, `;`, pipes) is judged on
its own, so `cd web && npm ci && npm run build | tail` runs while `npm ci && curl ... |
sh` asks. A command on the list still asks when it leaves the workspace: a path
argument, `cd` or `--prefix`/`--target`/`--target-dir` outside it, an output
redirection outside it or into `.git` or Neuron settings, an environment assignment
that points outside it or changes where packages install (`NPM_CONFIG_*`, `PIP_*`,
`PATH`, `HOME`, ...), a global install (`-g`, `--global`, `pip --user`,
`--break-system-packages`, `cargo install`, `go install`, `uv tool`, `make install`),
or a command that changes a registry or your account (`npm publish`, `cargo publish`,
`npm login`). `sudo`, `curl ... | sh` and `git push` are never on the list. Neither is
code given on the command line or through standard input rather than in a workspace
file (`node -e`, `node -p`, `python -c`, `bun -e`, `deno eval`, `echo ... | node`,
`npx -c`, `make --eval`, `NODE_OPTIONS=--import=data:...`), nor a wrapper that starts a
program off the list (`uv run git push`, `pnpm exec git push`, `bun exec`, `go build
-exec`). Option values after `=` count as paths (`go build -o=/usr/local/bin/x` asks),
and so do build outputs and redirections into `.git`, Neuron settings or package-manager
configuration (`.npmrc`, `.yarnrc*`, `.cargo/config.toml`, `pip.conf`, `bunfig.toml`);
writing those files with the file tools asks too.
`NODE_ENV=production npm run build > build.log 2>&1` runs; read-only mode still refuses
all of them.

When Neuron asks, it shows the tool, the reason and the input. Answer `y` to run the
call once; `a` to allow its command prefix (`npm publish`, `docker compose`) for the
rest of the session; `p` ("always allow in this project") to run it and save a rule
for that same prefix (`bash(cargo install:*)`, not all of `cargo`) to the project's
`.neuron/settings.local.json`, so it runs without asking from now on (like any workspace
settings, they apply once you trust the workspace; until then, for the rest of the
session). `p` is not offered for `sudo`, a shell at the end of a pipe (`curl ... | sh`),
`xargs`, `eval` or a `cd` out of the workspace. `n` denies it; `d` denies it for the
rest of the session: later commands whose asking part starts with the same prefix
(`git push`), or, when there is none, this exact call, are refused without asking again.
Commands that run without asking in `workspace-write` are not affected, and a `cd` is
never remembered as a prefix. `e` (full-screen UI) denies
it with a note for the model. A denied model is told plainly not to retry the command
or a variant. When standard output is redirected, the question goes to standard error.
When standard output and standard error are both redirected, nobody would see the
question, so the call is denied instead.

`neuron prompt` asks only when standard input is a terminal. In a script or CI
(standard input a pipe, a file or `/dev/null`) a call that needs approval is denied
without asking, the model is told that nothing ran, and questions from the model are
refused so it decides itself. After 5 denied calls the turn stops. A run that denied a
call ends with a warning naming the denied tools and exits with code 2. Each denial
names the `permissions.allow` rules that would let that call run, checked against the
call first: `"bash(npm:*)"` for `npm publish`, `"bash(docker:*)", "bash(npm:*)"` for
`docker build . && npm publish`, `"WebSearch"` for a web search. A command no prefix
rule covers gets a rule for that exact command (`"bash(npm publish > out.txt)"`), or
none when it is longer than one line or 160 characters. The end-of-run warning, and
its `allow_rules` field in JSON mode, lists every rule the denials named. Put them in
your user settings (`~/.neuron/settings.json`), because a script never gets the trust
question and an untrusted workspace's own allow rules are ignored. Full access
(`--permission-mode danger-full-access`) is named only as a last resort, for a
disposable environment.

`permissions` rules in settings can allow or deny tools and command prefixes in
advance, for example `{"permissions": {"allow": ["bash(git:*)"]}}`. `bash(git:*)`
allows a shell command when every part of it that needs approval starts with `git`,
so `git status; curl ...` still asks. Prefix rules count together: `bash(cargo:*)` and
`bash(npm:*)` allow `cargo test && npm run build`. No prefix rule covers a part that
writes a file through a redirection (`npm test > out.txt`), sets an environment
variable first (`CI=true npm test`), uses a command substitution or a PowerShell script
block, or runs a program named by a variable; `bash(npm test > out.txt)` allows that
exact command only. A deny rule wins when any part matches it.

Rules for MCP tools name one tool (`mcp__github__delete_repository`) or a whole server
(`mcp__github` or `mcp__github__*`, which also cover tools the server adds later). A
call through the `MCPTool` wrapper is checked as a call of the tool it names. A server
that a deny rule refuses whole is not started. See
[workspace servers, permissions and output](docs/DEFAULT_INTEGRATIONS.md#workspace-servers-permissions-and-output).

Choose the mode with `--permission-mode MODE`, switch inside a session with
`/permissions MODE`, or set a default in settings:

```json
{ "permissions": { "defaultMode": "read-only" } }
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

# a script that keeps to its own session
id=$(neuron --output-format json prompt "plan the change" | jq -r .session_id)
neuron --resume "$id" prompt "now make the change"
```

`latest` is the newest session with messages: a session left before its first prompt is
skipped. On a terminal, `--resume` without slash commands continues the session
interactively, in the full-screen UI when it is installed (the line REPL with
`--no-tui`, or from the home folder or a drive root without `--allow-broad-cwd`, where
it asks before it continues).
`--resume SESSION prompt "..."` works anywhere, scripts included: it runs one turn as
`neuron prompt` does, with the session's history, and saves the turn to the same file.
`-p "..."` does the same, and with nothing after it takes the prompt from standard input
(`echo "now add tests" | neuron --resume latest -p`); piped slash commands, one per line,
run as they do after `--resume` (`echo /status | neuron --resume latest -p`), and
without `--resume` they are refused rather than sent. With `--output-format json` its
stdout is the turn's JSON answer, as for `neuron prompt`, and its `session_id` is the
session's id: a script can continue the session it started by id rather than as
`latest`, which another run may have changed. Its `usage` and `estimated_cost` are that
run's alone; `session_usage` is the session's total, earlier runs included. `--model`, `--permission-mode`,
`--allowedTools`, `--reasoning-effort` and the other launch flags go before `prompt` or
`-p`, before or after `--resume`; without `--model` the turn runs on the model the
session used. Everything after `-p` is the prompt, so a flag such as `--model` or
`--output-format` there is refused rather than sent to the model, as are `--resume` and
`--continue` anywhere in a prompt.

One run at a time can continue a session. While a terminal session, the full-screen UI
or a script is on it, another run that would continue it (`--resume`, `/resume`,
`/session switch`, or `/compact` and `/clear --confirm` given to `--resume`) is refused
with "is in use by another neuron run" before it sends anything, instead of the two runs
overwriting each other's turns. Wait for the other run to end, or start a new session.
The locks for this are kept in `~/.neuroncli/locks` (under `NEURON_HOME` if set), one
per session file, so `--resume <file>` leaves nothing beside the file; a file that is
not a session is refused before it is locked.

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
| `/rewind [id [code\|chat\|both]]` | List the checkpoint saved with each prompt, or go back to one: the files, the conversation or both |
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
| OpenRouter | `OPENROUTER_API_KEY` (the gateway offers two of OpenRouter's free models; your key gives you the rest) |
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
| `managed-settings.json` in `C:\Program Files\Neuron` (Windows: the Program Files folder Windows reports), `/Library/Application Support/Neuron` (macOS) or `/etc/neuron` (Linux) | Managed settings an administrator sets for the whole computer; nothing overrides them (see [Connector policy](#connector-policy)) |

Later files override earlier ones. `/config` shows the merged result. Settings are
checked when they load: an unknown key or a value of the wrong type is an error that
names the file, the line and the closest known key. Commonly used keys:

| Key | Purpose |
| --- | --- |
| `model` | Default model |
| `permissions.defaultMode` | Default permission mode (`permissionMode` still works but prints a deprecation warning) |
| `permissions` | `allow`, `ask` and `deny` rules for individual tools |
| `provider` | Provider to use (see [Bring your own key](#bring-your-own-key)) |
| `maxIterations` | Model calls allowed in one turn (default 200); `NEURON_MAX_ITERATIONS` overrides it. An interactive turn that reaches it is paused, with its work kept: type `continue` (or press Enter in the full-screen UI) to go on. A `neuron -p` run stops with an error there |
| `workingSetTokens` | Conversation tokens to keep before older tool results become stubs and history is compacted (default 40000 on a plan with a token window, such as Free; none on other plans); `0` turns it off, and `NEURON_WORKING_SET_TOKENS` overrides it. See [The Free token window](#the-free-token-window) |
| `hooks` | Commands run before and after tool calls |
| `mcpServers`, `defaultMcpServers`, `disabledMcpServers` | MCP servers (`"type": "zerox"` is a [hosted connector](#hosted-connectors)) |
| `mcpServers.<name>.sandbox` | What a stdio MCP server may read, write and reach, and the inherited variables it sees (`fsRead`, `fsWrite`, `network`, `envAllow`); without it a catalog connector uses its entry's profile and any other server gets the workspace read-only with no network. See [the MCP sandbox](docs/DEFAULT_INTEGRATIONS.md#workspace-servers-permissions-and-output) |
| `mcpPolicy` | Which MCP servers may start, must start, and extra tool rules; read only from the managed settings (see [Connector policy](#connector-policy)) |
| `defaultSkills` | `false` leaves the bundled skills out of the system prompt |
| `toolCapableModels`, `autoToolModel`, `subagentModel`, `summaryModel`, `reviewerModel`, `providerFallbacks` | Models for particular jobs (see [Models](#models)) |
| `redactSecrets` | `false` stops replacing token-shaped secrets (API keys, tokens, private keys, passwords in URLs) with `[REDACTED:<kind>]` in tool output, the session file and telemetry; on by default, and the settings of a folder you have not trusted cannot turn it off |
| `sandbox.network.allow` | Hosts that `WebFetch` and shell commands may reach (`github.com`, `*.npmjs.org`, `*`, optionally with `:port`). `WebFetch` always enforces it; shell commands get proxy variables for a local proxy that only reaches those hosts, which macOS enforces and Linux and Windows treat as advice |
| `sandbox.maxProcesses`, `sandbox.memoryLimitMb` | Windows: the most processes a shell command may run at once (default 512) and its memory limit in MB (default none) |

Default skills and MCP servers are described in
[default integrations](docs/DEFAULT_INTEGRATIONS.md). `/mcp catalog` lists vetted
connectors and `/mcp add <id>` (or `neuron mcp add <id>`, which asks for secrets with
the input hidden) adds one; its secrets go to the OS keychain and the settings hold
`${secret:NAME}` (see [connector catalog](docs/DEFAULT_INTEGRATIONS.md#connector-catalog)).
Catalog connectors and the default servers start from verified installs in
`~/.neuron/mcp`, never through `npx` or `uvx`
(see [verified installs](docs/DEFAULT_INTEGRATIONS.md#verified-installs)). `/mcp list`
marks any other stdio server `unpinned`; `/mcp pin <server>` (or `neuron mcp pin
<server>`) records its package version and hashes, or its program's sha256, and it
starts from that from then on (see
[pinning other servers](docs/DEFAULT_INTEGRATIONS.md#pinning-other-servers)).

In the full-screen UI, `/mcp` (or a click on the status bar's connectors chip,
`⧉ 5/6`: servers up of those turned on, amber when one needs you, a lock when the
connector policy requires one) opens the MCP panel. Its **Installed** tab lists each
server with its transport, scope and state, and a badge after the name of one that
waits for you: `[blocked]`, `[needs approval]`, `[needs connect]` or `[unpinned]`.
Space turns a server on or off (not a required
one, marked with a lock), `a` approves a workspace server waiting for approval, `c`
opens the page that links a hosted connector, `l` signs in to a remote server that
needs it (`/mcp login <server>`), and `x` twice removes a server with its secrets.
Tab opens the **Add** tab: type to search the catalog, Enter picks a connector, then
fill its variables (secret ones are masked, and pasting works), name it, pick its
scope (`user`: every folder; `project`: this repository, shared; `local`: this folder,
not committed) with ←/→, and Enter adds it. A call of an MCP tool in the transcript
names its connector and, for catalog and hosted connectors, the tool's tier
(`github · merge_pull_request [destructive]`).

### Workspace trust

A cloned repository's own settings files (`.neuron.json`, `.neuron/settings*.json`
and the `.claw` equivalents) can run commands, so Neuron applies them only once you
trust the directory. The first time you start Neuron in a directory it asks; your
answer is saved in `~/.neuroncli/trusted_dirs.txt` (under `NEURON_HOME` if set).
A directory is also trusted when it sits under a `trustedRoots` entry in **your user
settings**; a repository cannot list itself. For headless runs, set
`NEURON_TRUST_WORKSPACE=1` only for checkouts you trust.

Until a directory is trusted, its settings keep only what restricts Neuron:
`permissions.deny` (added to your own deny rules), a read-only
`permissions.defaultMode` (or the deprecated `permissionMode`), and
`"defaultMcpServers": false` or `"defaultSkills": false`.
Hooks, MCP servers, allow and ask rules, model, aliases, provider and the other keys
are ignored, with a warning naming them. Once a directory is trusted, each MCP server
its settings (or its `.mcp.json`) add still waits for your approval
(`/mcp approve <server>`), and asks again when its command, arguments, environment,
headers or URL change. The full-screen UI's trust screen lists those servers with a
checkbox each (↑/↓ and space) and what each one runs: its command and arguments, or
its URL, with secret-looking values shown as `[REDACTED]` (environment and headers are
never shown). The ones you tick are approved as soon as the folder is trusted, but
only as shown: a server whose settings changed in between is not approved and waits
in `/mcp`, like the ones you did not tick. An engine too old to approve them from the
trust screen says so in the chat.
Even in a trusted directory, settings files
cannot set a full-access permission mode; pass `--permission-mode danger-full-access`
or put it in your user settings.

### Connector policy

Two policies decide which MCP servers may start, the first one winning:

- **Managed settings**, `managed-settings.json` in a system folder (the paths are in
  the table above). Neuron reads it only when no one but an administrator can have
  written it: the file, its folder and the system folder above must be owned by SYSTEM,
  Administrators or TrustedInstaller on Windows (root on macOS and Linux) and no other
  account may change them. Otherwise the file is ignored, with a warning, so an
  administrator who creates the folder should keep its default permissions. No
  environment variable moves the file. It loads after every other settings file,
  so nothing overrides it. Its `permissions` rules are added to everyone's rather than
  replacing them, and only there is `mcpPolicy` read (anywhere else it is ignored, with
  a warning).
- **Your Zero-X plan.** Each time Neuron reads your account (`/login`, `/usage`, the
  full-screen UI's account line) it keeps the plan's connector policy in
  `~/.neuron/policy-cache.json`, so the policy also applies after a restart and offline.
  A policy more than a day old is still used, with a warning. Signing out removes it.

```json
{
  "mcpPolicy": {
    "allow": ["catalog:*", "corp-*"],
    "deny": ["slack"],
    "required": ["codebase-memory"],
    "allowUnvetted": false,
    "allowStdio": true,
    "allowedRemoteHosts": ["*.corp.example"],
    "toolRules": {
      "deny": ["mcp__github__delete_repository"],
      "ask": ["mcp__github"]
    },
    "failClosed": false,
    "requireSandbox": true
  }
}
```

| `mcpPolicy` key | Meaning |
| --- | --- |
| `allow` | Only these may start: catalog ids (`github`), `catalog:*` (any catalog connector) or server-name patterns (`corp-*`). A catalog id matches only that connector as the catalog writes it, never a server that merely has that name |
| `deny` | Never start: the same forms; a catalog id also refuses a server of that name, one that runs the connector's package at another version (`npx -y @playwright/mcp@latest`, `python -m mcp_server_git`) and one on its remote host. A deny list is still a best effort, since a server can run the same code some other way: to be sure only approved servers run, use `allow` or `"allowUnvetted": false` |
| `required` | Always configured and on: catalog ids, or servers the managed settings define. A required server cannot be turned off (`/mcp disable`, the panel shows a lock, or `[req]` in a classic console) or removed, needs no workspace approval, and also starts in `neuron prompt` runs. A `permissions.deny` rule in your or a workspace's settings does not keep it from starting (it still refuses the tools it covers); only the managed settings' own deny rules block it |
| `allowUnvetted` | `false`: only catalog connectors may start |
| `allowStdio` | `false`: no local (stdio) servers |
| `allowedRemoteHosts` | Remote servers may reach only these hosts (`example.com`, `*.example.com`) |
| `toolRules` | `deny`, `ask` and `allow` rules added to everyone's permission rules; a deny rule always wins over an allow rule |
| `failClosed` | `true`: while the plan's policy has not been read in the last day (offline, signed out), only the servers the managed settings define or require start |
| `requireSandbox` | `true`: every stdio server runs in its sandbox; a `"sandbox": false` in user or workspace settings is ignored with a warning (the managed settings' own servers may still turn it off) |

A server the managed settings define is never refused by either policy. A refused
server shows as `blocked` in `/mcp` and the MCP panel, with the reason, and does not
start; `/mcp add` refuses a connector a policy would block. `/mcp` ends with the
policies in force.

Every MCP call, refused calls included, adds a line to
`~/.neuron/audit/mcp-YYYY-MM-DD.jsonl` (UTC day): tool calls, and resource requests
as the tools `resources/list` and `resources/read` (server `*` when a list asks every
server; a read hashes its URI). Each line has the time, session, server, tool,
catalog connector and catalog version, whether it was allowed and what decided it
(`rule`, `mode`, `user`, `session` or `hook`), how long it ran, the bytes in and out,
the sha256 of the arguments and an error class. Never the arguments or the output
themselves. The log stays on your computer.

### Hosted connectors

Hosted connectors are SaaS connectors (GitHub, Linear, Notion and others) that you link
once on the Zero-X dashboard (Connectors). Zero-X keeps their sign-in, so no token for
them sits on your computer; Neuron reaches them through the gateway with your Zero-X
sign-in (`neuron login`, or `NEURON_TOKEN` with an `nzx_` key in CI). They are off in
this release: set `NEURON_HOSTED_CONNECTORS=1` to try them. Free plans include none;
Pro and Max plans list theirs in the plan's connector policy.

With them on, each hosted connector your plan includes appears in `/mcp` as a
`managed` server of that name with transport `zerox`. Until you link it, the gateway
answers that it is not linked and the server shows `needs connect`:
`/mcp connect <server>` (or `neuron mcp connect <server>`) prints the page that links
it and opens it in a browser, and `c` on its row in the MCP panel does the same. It is
ready from the next turn after you link it: Neuron asks the gateway again at the start
of each turn and when the MCP panel opens, without a restart. If the gateway refuses
your sign-in, run
`neuron login` again. You can also add one yourself:

```json
{
  "mcpServers": {
    "github": { "type": "zerox", "id": "github" }
  }
}
```

A hosted connector only ever reaches the gateway's own `/mcp/<id>`; a `url` naming any
other address is refused, so your Zero-X sign-in never goes elsewhere. Its tools get the
tier the connector's catalog entry pins (read tools run without asking; others need
full access or your approval), and tools the catalog does not list need full access.
For destructive tools (deleting, merging, running workflows) the gateway also wants to
know that you approved the call, so Neuron asks you about every call of one, even in
full access, with an allow rule, or after "allow for this session", and tells the
gateway only about the call you approved. Where nobody can be asked (`-p` in a script)
such a call is not run. Turn one off like any other server (`/mcp disable <server>`).

## Environment variables

| Variable | Used by | Purpose |
| --- | --- | --- |
| `NEURON_TOKEN` | neuron | Gateway credential for this process |
| `NEURON_API_BASE` | neuron | Gateway URL (default `https://zero-x.live/v1`) |
| `NEURON_MODEL` | neuron | Model for this process |
| `NEURON_PROVIDER` | neuron | Provider for this process (see [Bring your own key](#bring-your-own-key)) |
| `NEURON_MAX_ITERATIONS` | neuron | Model calls allowed in one turn (default 200; interactive turns pause there, `-p` runs fail) |
| `NEURON_WORKING_SET_TOKENS` | neuron | Working set in conversation tokens; overrides `workingSetTokens` (`0` turns it off) |
| `NEURON_AUTO_COMPACT_INPUT_TOKENS` | neuron | Estimated context tokens at which older history is compacted automatically (default: worked out from the model's context budget) |
| `NEURON_FIRST_TOKEN_TIMEOUT_SECS` | neuron | Seconds to wait for a response to start, also after tool results (default 100 for the gateway; 90, and 60 after tool results, for a provider called directly) |
| `NEURON_DENIAL_LIMIT` | neuron | Denied tool calls after which a non-interactive `neuron prompt` run stops (default 5) |
| `NEURON_COMPLETION_CHECK` | neuron | `0` turns off the completion check: after a turn that changed files, Neuron asks the model once to check its changes against the request before it finishes |
| `NEURON_AUTO_CHECKS` | neuron | `0` stops the completion check from running the project's checks (`cargo check`, the typecheck script, `go vet`, `py_compile`, or `.neuron/checks.json`) itself; they run only when your permission settings allow them without asking |
| `NEURON_REVIEW` | neuron | `1` adds a reviewer to the completion check: a read-only sub-agent that compares the turn's diff with the request and reports problems |
| `NEURON_REVIEW_MODEL` | neuron | Model for the reviewer (default: the session's model) |
| `NEURON_EXPERIENCE` | neuron | `0` turns off the experience library (`experience.json` in the settings directory): per-model counts of repaired tool calls, missed edits and similar, which add notes to that model's system prompt once they recur |
| `NEURON_CONFIG_HOME` | neuron | Settings directory (default `~/.neuron`) |
| `NEURON_HOME` | neuron | Directory for the login file and caches (default `~/.neuroncli`); setting it also keeps the login out of the OS credential store |
| `NEURON_CREDENTIAL_STORE` | neuron | `file` or `keychain`: force where the login is kept |
| `NEURON_TRUST_WORKSPACE=1` | neuron | Trust the current directory in a headless run (only for checkouts you trust) |
| `NEURON_HOSTED_CONNECTORS=1` | neuron | Turn on hosted connectors (off by default; see [Hosted connectors](#hosted-connectors)) |
| `NEURON_NO_TUI=1` | neuron | Use the line REPL instead of the full-screen UI |
| `NEURON_TUI_ASCII` | neuron-tui | `1` draws with the classic-console character set everywhere, `0` with the Unicode one |
| `NEURON_AMBIGUOUS_WIDE` | neuron-tui | `1`: the terminal draws East Asian ambiguous-width characters (`│ ● ○`) two columns wide, so the status bar measures them so; `0`: one column. Unset, a Chinese, Japanese or Korean locale counts as `1` |
| `NEURON_VERSION` | install.sh, install.ps1 | Release to install (default: the latest release; a prerelease such as `7.0.0-alpha.5` must be named) |
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
| npm | `npm install --global @zero-x.live/neuron` (the alpha is published as npm's `latest`; `npm view @zero-x.live/neuron version` shows which version that is) | `npm uninstall --global @zero-x.live/neuron` |
| pip | `pip install --upgrade --pre neuroncli` (the alpha is a pre-release on PyPI, so pip needs `--pre`) | `pipx uninstall neuroncli` or `pip uninstall neuroncli` |

Neuron keeps your login in the OS credential store or `~/.neuroncli/`, and your
settings in `~/.neuron/`. Run `neuron auth logout` before deleting them so the
session is revoked on the gateway and removed from the credential store too.

The npm package downloads `neuron` (about 17 MB) and `neuron-tui` (about 110 MB)
during `npm install`, without showing progress, into its own `bin/native` folder, so
uninstalling the package removes them; when the network fails during the install, npm
downloads them into that folder on first use instead. pip downloads `neuron` the first
time you run it and `neuron-tui` the first time the full-screen UI opens, into a
per-version cache: `~/.cache/neuroncli` (Linux), `~/Library/Caches/neuroncli` (macOS)
or `%LOCALAPPDATA%\neuroncli` (Windows); delete it after uninstalling. npm uses a
binary from that cache when its `bin/native` folder has none, and downloads into the
cache only when that folder is read-only.
With the installers or pip, the two default MCP servers start through `npx -y`, which
needs Node.js; the npm package installs them with Neuron.

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
