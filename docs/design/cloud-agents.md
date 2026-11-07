# Design: cloud and background agents

Status: proposal (2026-11). Nothing here changes the Zero-X gateway; items marked
**gateway** are what a later, separate zero-x.live change would add.

## Goal

Start a Neuron task that keeps running when the laptop sleeps: from the TUI
(`/cloud "fix the flaky test"`), from the phone, from an issue or PR comment, from a
schedule. Watch it live from the TUI or the phone, steer or stop it, and get the diff
or PR at the end, with a hard cost cap.

## Building blocks that exist

| block | where | role in cloud agents |
|-------|-------|----------------------|
| headless `neuron prompt` | `rusty-claude-cli` | the agent process on a runner; denies what nobody can approve |
| `--output-format stream-json` | `stream_json.rs`, [schema](../stream-json.md) | the event stream a runner forwards |
| engine protocol (`UiEvent` / `EngineCommand`) | `neuron-tui/src/protocol.rs` | the same events the TUI already renders; commands to steer |
| sessions (JSONL, resume, fork) | `runtime/src/session.rs` | the durable state of a task |
| checkpoints | `runtime/src/checkpoints.rs` | per-turn file snapshots, rewind |
| artifact store | `runtime/src/artifacts.rs` | large outputs and reports, content-addressed |
| share relay | `wss://relay.zero-x.live/v1/relay` (WP4 contract) | end-to-end encrypted frames between a host and phones |
| GitHub Action | `action.yml`, [docs](../github-action.md) | the first runner: GitHub-hosted, per comment |
| OTel export | `telemetry::otel` | spans and token metrics per turn, model request, tool call |
| Zero-X gateway | zero-x.live | auth, model routing, quotas, key pools |

## Architecture

```text
 TUI / phone / GitHub comment / schedule
          │  create task (prompt, repo@ref, model, mode, budget)
          ▼
 Zero-X gateway  ── task queue (gateway) ──►  runner pool
   auth, quotas,                              ├─ GitHub Actions (exists: action.yml)
   budget ledger                              ├─ container runner (Containerfile)
          ▲                                   └─ self-hosted runner (user's own box)
          │ usage + cost per request                    │
          │                                             ▼
          │                               sandbox: fresh clone, neuron prompt
          │                               --output-format stream-json
          │                                             │ events (stream-json lines)
          │                                             ▼
          └──────── share relay (host role = runner) ───► TUI / phone (client role)
```

### Runners

A runner is anything that can run `neuron prompt` in a fresh checkout and forward its
stdout. Three kinds, in order:

1. **GitHub Actions** (shipped as `action.yml`): no infrastructure, per-repo secrets,
   results as comments. Limited to the 6 h job limit and to GitHub repositories.
2. **Container runner**: the existing `Containerfile` image plus a small supervisor
   (`neuron runner`, future) that pulls tasks from the gateway queue, clones `repo@ref`
   into an empty volume, runs the agent and pushes the branch. Hosted by Zero-X or by a
   team on its own Kubernetes/ECS.
3. **Self-hosted**: the same supervisor on a user's workstation or VM, registered with
   a runner token, for private networks.

### Isolation

- One task, one container (or VM), one fresh clone; nothing shared between tasks.
- Non-root user, read-only root filesystem, the work tree on a scratch volume.
- Network egress allowlist: the gateway, the git host, the package registries the repo
  declares; everything else blocked (the in-process Linux `unshare` sandbox stays on as
  a second layer).
- Credentials: a short-lived, task-scoped gateway token (never the user's login), and a
  git token scoped to the one repository with push to `neuron/*` branches only. Both are
  injected as environment variables, which Neuron already strips from shell commands,
  hooks and plugins.
- Permission mode is chosen at task creation; `danger-full-access` only inside the
  container boundary, never on self-hosted runners without explicit opt-in.

### Session storage

- The runner writes the session JSONL, checkpoints and artifacts under
  `NEURON_HOME=/work/.neuron-home` as today.
- On every turn end and on exit it uploads them to object storage keyed by
  `account/task/session` (**gateway**: signed upload URLs). Artifacts are already
  content-addressed, so uploads dedupe by hash.
- `neuron --resume <session>` on a laptop downloads the session (**gateway**: signed
  download URL) and continues locally; the cloud task can likewise resume a local
  session that was uploaded.
- Retention follows the account plan; artifacts and checkpoints expire first.

### Streaming to the TUI and the phone

The runner joins the share relay in the **host** role exactly like `neuron --share`
does on a laptop (WP4 contract): one WebSocket to `wss://relay.zero-x.live/v1/relay`,
AES-256-GCM sealed frames, the relay's 512-frame replay buffer for reconnects, push
notifications (`finished`, `needs_me`). The frames carry engine protocol events, which is
what stream-json lines already are, so the TUI attaches to a cloud task with the code it
uses for a local engine, and the phone needs no new screens.

Steering goes the other way as `EngineCommand`s through the relay: `Interrupt`,
`Submit` (a follow-up turn), and permission replies. A cloud task normally runs with no
approver; with a phone attached it can instead *ask* (`PermissionRequested`) and wait up
to a timeout before denying, which is the `needs_me` push.

### Cost limits

- Each task carries a budget (USD and/or tokens) and a wall-clock limit set at creation,
  capped by the plan.
- Enforcement in three places, cheapest first:
  1. the runner reads `request_usage` events and stops the agent (`Interrupt`) at 90 %;
  2. the gateway's budget ledger refuses model requests for the task's token once the
     budget is spent (**gateway**: per-token budget, it already meters usage per key);
  3. the supervisor kills the container at the wall-clock limit.
- `NEURON_MAX_ITERATIONS` and `NEURON_DENIAL_LIMIT` bound runaway loops independently
  of money.
- The final `result` line reports `usage` and `estimated_cost`; OTel metrics
  (`neuron.tokens`) feed dashboards and alerts when the team runs a collector.

### Triggers

- Comments: the GitHub Action today; later a GitHub App posting to the gateway queue,
  so no workflow file is needed.
- Schedules and webhooks: **gateway** endpoints that enqueue a task template.
- Slack: a slash command that enqueues a task and replies with the relay link.

## Phases

1. Shipped in this lane: stream-json, GitHub Action, artifact store, OTel export.
2. `neuron runner` supervisor + Containerfile hardening + relay host mode for runners
   (reuses `--share`).
3. **gateway**: task queue, task-scoped tokens, budget ledger, session object storage.
4. TUI `/cloud` and task list; phone task list; GitHub App.

## Open questions

- Who pays for hosted runner compute: plan minutes, or bring-your-own runner only at
  first?
- Should a task that needs approval park (container stopped, session uploaded) instead
  of holding a runner while it waits?
- Pushing branches vs. opening PRs directly: PRs need a GitHub App identity.
