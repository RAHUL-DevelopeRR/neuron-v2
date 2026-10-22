# Neuron desktop and mobile apps: engine audit, gaps and plan

Status: draft, 2026-10-22. Branch `feat/neuron-apps`, stacked on PR #10 (`feat/tui-freebuff-port`),
because `rust/crates/neuron-tui/src/protocol.rs`, `neuron engine --stdio` and the mock scripts
exist only there so far.

## 1. Target

One shared coding-agent engine and three front ends on it. Under them sit the extensions.

```
                 ┌──────────────── shared engine: neuron (Rust) ────────────────┐
                 │ planning · context · tool execution · multi-step tasks ·     │
                 │ memory · permissions                                          │
                 └───────┬───────────────────────┬──────────────────────┬───────┘
          neuron engine --stdio          neuron engine --stdio      remote protocol
             (JSON lines)                (Tauri sidecar)          (host or relay, WebSocket)
                 │                               │                        │
              TUI (Bun/OpenTUI)        Desktop (Tauri 2 + React)    Mobile (Expo/React Native)
              exists, PR #10            IDE-like workspace           remote agent control
                 └───────────────────────┴──────────────────────┴───────┘
       extensions: MCP servers · plugins · skills · Git and GitHub · model providers · memory and indexing
```

"Like Claude Code" means the same capabilities in Neuron's own code and brand: plugins,
connectors, skills and MCP, a permission dialog, diffs, a terminal, and remote control. No front
end reimplements the agent. Each one speaks the engine's protocol.

## 2. What the engine has today

Paths are relative to `rust/crates/`. This comes from reading the code on 2026-10-22, at
`feat/tui-freebuff-port` 216bddd.

### 2.1 Front-end protocol (`neuron-tui/src/protocol.rs`, `rusty-claude-cli/src/engine.rs`, `engine_stdio.rs`)

- **Commands:** `submit` (text and attachments), `slash`, `set_model`, `list_models`,
  `interrupt` and `shutdown`. Permission replies are `allow_once`, `allow_for_session` or
  `deny` with a note.
- **Events:** `ready`, `status`, `session_starting`, `sign_in_code`, `models`,
  `text_delta`/`text_block_end`, `reasoning_*`, `tool_call_started`, `tool_result` (with ids),
  `usage`, `permission_requested`, `notice`, `turn_finished` and `error`.
- **Slash commands** the engine runs: help, status, cost, stats, context, compact, model,
  permissions, clear, resume, plan, login, logout and exit. Any other command is refused with
  "not available in the full-screen UI yet" (`engine.rs:996`).
- **Single client.** There is no authentication, no sequence numbers and no history. A reconnect
  loses everything. Pending permission requests and questions are not re-sent.
- **AskUserQuestion** reaches the UI only as a notice that starts with `Question:`. The next
  `submit` is taken as the answer (`engine.rs:602`).
- **Workspace trust.** An untrusted workspace runs without its own settings, hooks and MCP
  servers. There is no command to trust one.

### 2.2 MCP (`runtime/src/mcp*.rs`, `runtime/src/config.rs`)

- **Config:** `mcpServers` in user, project and local `settings.json` (also the `.claw*` and
  `.neuron*` variants). Untrusted workspaces drop project servers.
- **Parsed server types:** stdio, sse/http (with headers, headersHelper and OAuth fields), ws,
  sdk and claudeai-proxy.
- **Implemented transport: stdio only.** `McpServerManager::from_servers` marks every other type
  "not supported" (`mcp_stdio.rs:806-832`).
- **Methods:** initialize, tools/list, tools/call, resources/list and resources/read. There are
  no prompts, sampling, roots, elicitation or `list_changed`.
- **Bundled servers:** codebase-memory and playwright, started with `npx`. Turn them off with
  `defaultMcpServers:false` or `disabledMcpServers`.
- **How the model sees them:** each tool becomes `mcp__<server>__<tool>`, offered after
  `ToolSearch` finds it, plus the `MCPTool`, `ListMcpResourcesTool` and `ReadMcpResourceTool`
  wrappers.
- **Startup:** in background in the engine. Per-server status reaches only the model, never the
  user.
- **CLI:** `neuron mcp list|show` (config only) and `neuron mcp serve` (Neuron's tools as an
  MCP server).
- **Dead code:** `global_mcp_registry` (the `MCP`/`McpAuth` tools) and `McpLifecycleValidator`.

### 2.3 Plugins (`plugins/src/lib.rs`)

- **Manifest:** `plugin.json` or `.claude-plugin/plugin.json`. Fields: name, version,
  permissions, defaultEnabled, hooks (PreToolUse, PostToolUse, PostToolUseFailure), lifecycle
  (Init, Shutdown), tools (subprocesses) and commands.
- **Management:** install from a path or a git URL, enable, disable, uninstall and update, under
  `~/.neuron/plugins/`. Available as `neuron plugins …` and the REPL's `/plugin`.
- **The engine loads them**, but the protocol cannot list or manage them.
- **Claude Code fields are rejected:** skills, mcpServers, agents, glob commands and other hook
  events (`lib.rs:1651`). Claude Code plugins do not install.
- **Parsed `commands` are never dispatched.**
- **No marketplace:** "marketplace" is only an id suffix.
- **`bundled_root()` is the compile-time `CARGO_MANIFEST_DIR`**, so it is missing in an installed
  binary.
- **`plugin_lifecycle.rs` is unused.**

### 2.4 Skills (`commands/src/lib.rs`, `tools/src/lib.rs`, `runtime/src/default_skills.rs`)

- **Format:** `<root>/<name>/SKILL.md` (only `name` and `description` are read), or legacy
  `commands/<name>.md`.
- **Discovery roots:** ancestor `.claw`, `.omc`, `.agents`, `.codex` and `.claude` folders,
  `~/.neuron/skills`, and others.
- **14 bundled skills**, loaded on demand through the `Skill` tool.
- **The system prompt indexes only the bundled skills**, so the model never hears about project
  or user skills.
- **`.neuron/skills` in a project is not a root.**
- **`/skills list` and the Skill tool disagree on Windows:** one reads `HOME`, the other falls
  back to `USERPROFILE`.
- **Hard-coded `/home/bellman/...` roots** (`tools/src/lib.rs:3419`).
- **No frontmatter beyond name and description:** no `allowed-tools`, `model`, `$ARGUMENTS` or
  `disable-model-invocation`.

### 2.5 Hooks (`runtime/src/hooks.rs`)

- **Events:** PreToolUse, PostToolUse and PostToolUseFailure only, as a flat list of commands.
- **Interface:** JSON on stdin plus env vars. Exit code 2 denies. Stdout JSON can carry
  `decision`, `hookSpecificOutput.permissionDecision`, `updatedInput` and `additionalContext`.
- **Missing:** matchers, timeouts, and the UserPromptSubmit, SessionStart/End, Stop,
  SubagentStop, PreCompact and Notification events. There is no `/hooks`.
- **`plugins/src/hooks.rs` duplicates the runner** and is unused.

### 2.6 Slash commands, agents, planning

- **Slash commands:** about 140 specs, many of them stubs. Custom `commands/*.md` files are
  reachable only as skills, never as `/name`, and there is no command list a UI could autocomplete
  from.
- **`Agent` tool:** real sub-agent threads with fixed types. The `.toml` agent definitions that
  `/agents` lists are never used. Claude Code `.md` agents are not parsed. Sub-agents get no
  approval path, so their escalations are silently denied.
- **Plan mode:** `/plan` switches to read-only. `EnterPlanMode`/`ExitPlanMode` write
  `settings.local.json` and are not connected to it. There is no plan approval step.
- **TodoWrite** saves `.clawd-todos.json`. There are no todo or sub-agent events.
- **Task, Team, Cron and Worker tools** are in-memory bookkeeping only.
- **Background bash** throws away its output.

### 2.7 Git and GitHub

- **Context:** branch, recent commits and staged files go into the system prompt once, at the
  start of the session, and are never refreshed. `status.git` gives branch and change counts.
- **`/commit`, `/pr` and `/issue`** only print a report. `/diff`, `/branch` and `/rewind` are
  line-REPL-only or stubs.
- **No `gh`/GitHub API, PR, worktree or commit tools.**
- **Unused:** `branch_lock.rs`. The stdio engine never runs `stale_base`.

### 2.8 Model providers

- **Providers:** Zero-X (default when signed in), OpenAI, OpenRouter, Azure, Anthropic, xAI and
  Local, with OpenAI-compatible routing.
- **Model catalog:** only the gateway publishes one, so `list_models` is empty for every other
  provider.
- **Auth:** Zero-X device-code login, stored in the keychain or a 0600 file. Other providers read
  env vars or a trusted workspace's `.env`.
- **The protocol cannot show or change the provider**, or enter a key.

### 2.9 Memory and indexing

- **Instruction files:** NEURON.md, AGENTS.md and CLAUDE.md (and local variants), from the root
  down to the cwd, capped at 4k characters each and 12k in total.
- **`repo_map.rs`:** a regex symbol scan of up to 150 files, added to the system prompt.
- **Sessions:** JSONL files under `.neuron/sessions/`, with resume and fork. Compaction is
  heuristic.
- **Missing:** a persistent memory store, `/memory`, `/init` in the engine, an embeddings or
  search index, and session listing or transcript replay through the protocol. `lsp_client.rs` is
  a placeholder.

### 2.10 Permissions

- **Modes:** read-only, workspace-write, danger-full-access, prompt and allow.
- **Rules:** allow, deny and ask. Sensitive paths need approval.
- **"Allow for session"** works by command prefix or web domain, and is kept in memory only.
- **The sandbox works on Linux only (`unshare`)**; Windows runs commands without isolation.
- **`neuron mcp serve` runs tools with no permission checks.**

### 2.11 Remote control

- **None in the engine:** no HTTP, WebSocket or relay server, and no way to attach to a running
  session.
- **`remote.rs` and `worker_boot.rs` are unrelated:** environment parsing and in-memory state.

## 3. Gaps, by what the apps need first

| # | Gap | Needed by | Phase |
|---|-----|-----------|-------|
| G1 | Fan-out to several clients, sequence numbers, a snapshot on attach, pairing | mobile, desktop + phone together | done in `ui/neuron-client` host (TS); E1 moves it into the engine |
| G2 | Protocol: list sessions, resume from the UI, transcript on attach | desktop session list, mobile | E1 |
| G3 | Protocol: trust the workspace from the UI | desktop, TUI | E1 |
| G4 | Structured events: questions, todos, sub-agent start/finish, background tasks | all UIs | E1 |
| G5 | Protocol: extensions inventory (MCP servers and their status, plugins, skills, commands, agents) | desktop Extensions panel | E2 |
| G6 | MCP over streamable HTTP and SSE, with OAuth (PKCE, dynamic registration), `.mcp.json` | connectors, Claude-Code parity | E3 |
| G7 | MCP management (add, remove, enable, restart), per-server status, prompts as slash commands | desktop, CLI | E3 |
| G8 | Claude-Code-compatible plugins: skills, agents, commands, mcpServers, hooks.json; marketplaces | plugins | E4 |
| G9 | Skills: index project and user skills, `.neuron/skills`, full frontmatter, `$ARGUMENTS`, fix HOME/USERPROFILE and `/home/bellman` | skills | E4 |
| G10 | Custom slash commands from `commands/*.md`; plugin commands dispatched | all UIs | E4 |
| G11 | Hooks: matchers, timeouts, the missing events | plugins, policy | E5 |
| G12 | Agents from `.md` definitions; an approval path for sub-agents | agents | E5 |
| G13 | Git: diff, stage, commit, branch, PR through `gh`; refresh git context each turn; worktrees | desktop source-control pane | E6 |
| G14 | Memory: persistent store and `/memory`; real LSP; optional index | memory | E7 |
| G15 | Providers through the protocol: show, switch, store a key; catalogs beyond the gateway | desktop settings | E7 |
| G16 | Engine-side remote: `neuron engine --share` to the relay, E2E encryption, push for approvals | mobile in production | R1–R2 |

## 4. Stacks (approved by Rahul on 2026-10-22)

- **Shared client: `ui/neuron-client`.** Pure TypeScript with no runtime dependencies, in this
  branch now.
  - `protocol.ts` mirrors `protocol.rs`. A test reads protocol.rs and fails when the variants
    drift.
  - `session.ts` is one reducer that turns engine events and the user's actions into a
    transcript: messages, reasoning, tool calls with state, permission requests tied to their
    call, questions, sign-in and usage.
  - `tools.ts` and `diff.ts` give each tool call a kind, a title, the diff it will make (from its
    input), the diff it made (from the engine's `diff`) and its output.
  - `controller.ts` holds a session over a transport, with a `useSyncExternalStore`-shaped store.
  - `remote.ts` is the remote protocol and a WebSocket transport that runs in browsers and React
    Native.
  - `node/` (Bun only) has a stdio transport, the host (one engine, many WebSocket clients) and
    mock-service helpers.
  - The TUI could later import the protocol from here instead of keeping its own copy. That is a
    follow-up for the TUI session; this branch does not touch `ui/neuron-tui`.
- **Desktop: `ui/neuron-desktop`, Tauri 2 + React 19 + Vite.**
  - The Tauri Rust side spawns `neuron engine --stdio` as a sidecar and passes JSON lines through
    Tauri events and commands, so no port is open.
  - Native commands cover the file tree (read-only, inside the workspace) and a PTY terminal
    (portable-pty, xterm.js).
  - `src-tauri` is a Cargo project of its own, so the engine workspace and its CI never build
    Tauri.
  - In a browser (`vite dev`) the same UI talks to the Bun host instead. That is how it is tested
    against the mock engine without the Tauri shell.
- **Mobile: `ui/neuron-mobile`, Expo (React Native) + expo-router.**
  - A remote control: pair with a host (URL and token), watch the transcript live, answer
    permission requests, send prompts, interrupt.
  - It uses the same reducer as the desktop, so both show the same session.
  - Previewed with react-native-web. Native builds need EAS or the Android/iOS SDKs, which come
    later and need Rahul's OK.

Disk: C: had 3.8 GB free on 2026-10-22. A Tauri debug build needs 1.5–2.5 GB of `target/`
(about 0.7 GB without debuginfo), and Expo needs about 0.5 GB of `node_modules`.

## 5. Remote protocol and the relay

### Remote protocol 1 (`ui/neuron-client/src/remote.ts`)

These are JSON text frames over WebSocket.

**Client to host:**
- `hello {protocol, token, client: {name, kind}}`: the token goes in the first frame, never in
  the URL
- `send {message}`: any `ClientMessage`, except that remote clients cannot `shutdown`
- `rpc {id, method, params}`: for now `fs.list` and `fs.read`, both read-only inside the
  workspace
- `ping`

**Host to client:**
- `welcome {host, clients}`
- `snapshot {seq, state}`: the reduced session, on every attach and reconnect
- `action {seq, action}`: engine events, plus the clients' own prompts and permission answers, so
  every client shows the same transcript
- `rpc_result`
- `clients {count}`
- `error {message, fatal}`
- `pong`

**Rules:**
- The first answer to a permission request wins; the host drops the later ones.
- Bad tokens close the socket with 4401. A frame before `hello` closes it with 4400.
- Browser origins other than local ones get a 403.

### The host today (`ui/neuron-client/src/node/host.ts`, `bun run host [--mock] [--lan]`)

- It binds 127.0.0.1, or the LAN with `--lan`.
- The token is random each run. It is written to a 0600 file and printed only with
  `--show-token`.
- The engine runs as the user, on the user's machine. The phone connects to that machine directly
  over the LAN, which works only when both are on one network that both trust.

### The relay (zero-x.live: described here, not built here)

The relay lets a phone reach the user's machine from anywhere, with no inbound port open on the
machine.

1. **Endpoints.**
   - `GET /relay/host` (WebSocket) for the machine.
   - `GET /relay/client` for the phone.
   - Both authenticate with the user's `ses_` session in an `Authorization` header (or the first
     frame, if a browser WebSocket can't send headers), as `/v1/*` does.
   - `nzx_` API keys must not be accepted here.
2. **One Durable Object per user, `RelayRoom`.**
   - It keeps the host sockets and client sockets, routes frames between them, and keeps nothing
     on disk.
   - With WebSocket hibernation, idle rooms cost nothing.
   - Per-user limits: about 3 hosts, about 5 clients, a 1 MB frame size, and frame-rate caps.
3. **Pairing.**
   - The machine registers a host with a name and a public key, and gets back a `host_id`.
   - The phone lists the user's hosts and picks one.
   - For end-to-end encryption, the machine shows a QR code holding the host's public key and a
     one-time code. The phone and the machine derive a shared key with X25519 and HKDF and seal
     every frame with XChaCha20-Poly1305, so the relay sees only ciphertext and routing headers
     (`{to, from, n}`).
4. **Events the gateway can add.**
   - A push notification ("Neuron needs your approval") when a `permission_requested` arrives
     and no client is attached. The frame is encrypted, so the host sends a separate, contentless
     `notify` frame.
   - A session list for the phone.
5. **Contract.** The relay belongs in `docs/GATEWAY_CONTRACT.md` (both copies) as section 8. It
   also needs a CLI obligation: `neuron engine --share` (phase R1) connects outbound to
   `/relay/host`.

## 6. Phases

### Phase A: this branch (draft PR)

- [x] The engine audit and this plan.
- [x] `ui/neuron-client`: protocol, reducer, diffs, controller, stdio transport, host, remote
  transport, and PTY terminals. 48 tests, including 5 against the real engine and the mock
  service: a turn with a write, approve, deny, parallel calls, and two clients steering one
  engine.
- [x] `ui/neuron-desktop` MVP (installs approved by Rahul, 2026-10-22).
  - chat with streaming, the permission dialog, tool cards with diffs
  - a file tree, a Changes panel, a terminal pane, the model picker, sign-in (device code), a
    status bar
  - 13 `bun test`s: 11 component tests, and 2 end-to-end tests of the whole app on the real engine
    and the mock
  - 7 `cargo test`s for `src-tauri`, one of them a real approval turn through the bridge on the
    mock
  - Not yet driven end to end inside the Tauri window. The app starts and its window starts the
    engine. WebView2's DevTools port never opened on this machine, so the full turn was checked
    in the browser build, which runs the same UI over the host.
- [x] `ui/neuron-mobile` skeleton.
  - pair, watch, approve or deny, send, interrupt
  - 4 `bun test`s
  - checked in react-native-web against the mock, including a reconnect while an approval was
    pending

### Phase E1: protocol for many front ends (engine, Rust)

- `list_sessions`, then `sessions {items}`.
- `resume {session}` from the UI.
- `get_transcript`, so an engine can rebuild a client's view after `--resume`.
- `trust_workspace`, after which the UI restarts the engine.
- Structured events, keeping the notice as a fallback for older UIs:
  - `question {id, text, options}` and `answer {id, text}`
  - `todos {items}`
  - `subagent_started` / `subagent_finished`
  - `task_output` for background bash
- Tests in `engine.rs` against the mock service, and in `ui/neuron-client`.

### Phase E2: extensions inventory

- `list_extensions` returns `extensions {mcp_servers: [{name, transport, status, tools, error}],
  plugins, skills, commands, agents}`.
- `mcp_server_status` events as servers come and go.
- The desktop Extensions panel reads it.

### Phase E3: MCP and connectors

- A streamable HTTP client, plus SSE for older servers, with OAuth 2.1 (PKCE, dynamic client
  registration, refresh), tokens in the credential store and `headersHelper`.
- Read `.mcp.json`, and import from `~/.claude.json`.
- `neuron mcp add|remove|enable|disable|restart` and the matching protocol commands.
- MCP prompts become slash commands. Handle `list_changed`, roots and elicitation (the UI asks the
  user).
- **Connectors** are hosted MCP integrations listed by zero-x.live.
  - The gateway does each service's OAuth and keeps the tokens.
  - The engine connects to `https://zero-x.live/mcp/<connector>` over streamable HTTP with the
    user's `ses_`.
  - Users enable connectors on the website or in the desktop app.
- The dead `global_mcp_registry` and `McpLifecycleValidator` go.

### Phase E4: plugins, skills, commands

- **Plugin manifests in the Claude Code layout:** `skills/`, `agents/`, `commands/`, `hooks/hooks.json`
  and `.mcp.json`, and `mcpServers` in the manifest. Plugins also contribute skills, agents,
  commands and MCP servers.
- **Marketplaces:** a git repository with `marketplace.json`.
  - `neuron plugins marketplace add|list|remove`
  - install by `name@marketplace`
  - protocol commands to list, install, enable and disable
- **Fixes:** `bundled_root`, and drop `plugin_lifecycle.rs`.
- **Skills:**
  - index project and user skills in the prompt (a name and one line each, within a budget)
  - add `.neuron/skills` as a root
  - frontmatter: `allowed-tools`, `model`, `disable-model-invocation` and `argument-hint`, with
    `$ARGUMENTS`
  - one home-directory resolver shared by the commands crate and the tools crate
  - remove the `/home/bellman` roots
- **`commands/*.md` and plugin commands dispatch as `/name`**, and the protocol publishes the
  command list for autocomplete.

### Phase E5: hooks and agents

- **Hooks:**
  - `{matcher, hooks: [{type: command, command, timeout}]}`, with the old flat list still read
  - UserPromptSubmit, SessionStart, SessionEnd, Stop, SubagentStop, PreCompact and Notification
  - `/hooks` lists them
- **Agents:**
  - `.md` definitions with frontmatter (name, description, tools, model) and a body that is the
    system prompt, from `.neuron/agents`, `~/.neuron/agents` and plugins
  - the Agent tool uses them
  - sub-agents ask through the parent's permission prompter

### Phase E6: Git and GitHub

- **Tools** for git status and diff, stage, commit (with a message the user can edit in the
  desktop app), branch, and worktree add or remove.
- **GitHub through `gh`:** PR create and view, issues, checks. It is permission-gated.
- **The git context is refreshed every turn.**
- **The desktop app gets a source-control pane:** changed files, a diff viewer, stage and commit.

### Phase E7: memory, indexing, providers

- **Memory:** a persistent store (project and user `MEMORY.md` with an index, `/memory`, and
  "remember this" from the UI), plus `/init` in the engine.
- **Indexing:** a real LSP client (diagnostics, go to definition for the model), and
  codebase-memory as the default index when installed.
- **Providers through the protocol:**
  - show which provider is active and switch it
  - store a key in the keychain; the UI sends the key once and never reads it back
  - catalogs for OpenAI-compatible providers (`/v1/models`)

### Phase R1–R2: remote in the engine, and the relay

- **R1:**
  - Move the host into the engine as `neuron engine --stdio --share`, or `neuron serve`
    (headless), so the TUI and the desktop app can share their session with the phone.
  - It needs a WebSocket client in Rust (tokio-tungstenite) and the E2E encryption above.
  - The Bun host stays as the reference and test fixture.
- **R2:** the zero-x.live relay (section 5), pairing by QR code, and push notifications for
  approvals.

### Phase P: packaging

- Tauri bundles in `release.yml` for Windows MSI/NSIS, macOS dmg (signed and notarized) and Linux
  AppImage/deb.
- Expo EAS builds for the stores, and over-the-air updates.
- Each needs Rahul's OK on the accounts and certificates.

## 7. Testing

- **`ui/neuron-client`:** `bun test`. The unit tests cover the reducer (from recorded engine
  event sequences), diffs, the protocol drift check and the host (tokens, origins, first answer
  wins, snapshots, fs bounds). The integration tests need `cargo build -p rusty-claude-cli -p
  mock-anthropic-service` and are skipped without it.
- **Test homes turn the bundled MCP servers off.** `npx` would otherwise download Playwright and
  codebase-memory into each fresh home (60–350 MB per run).
- **The desktop and mobile apps** reuse the mock-engine host: `bun run host --mock` in
  `ui/neuron-client`.
- **Engine phases** add Rust tests beside the existing `engine.rs` mock-service tests.

## 8. Decisions for Rahul

1. Done: the installs were approved, and the PR stacks on #10.
2. Disk: C: is nearly full. This worktree's `rust/target` (about 1.9 GB) and
   `ui/neuron-desktop/src-tauri/target` (about 1.4 GB) can be rebuilt.
3. Relay: E2E encryption from day one (recommended), or TLS-only first.
4. Connectors: gateway-hosted OAuth (recommended, so tokens never reach the user's machine), or
   OAuth done by the engine.
5. Does the mobile app need to start new sessions on the machine (a host that can spawn engines),
   or only control a session that is already running? The skeleton only controls a running one.
