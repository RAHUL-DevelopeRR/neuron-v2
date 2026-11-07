# Triggers: GitHub and Slack events that start a Neuron run

Status: design only (2026-11-02). Nothing here is implemented. No change to the Zero-X gateway is
part of this design; where it would help, it is listed under "Later".

## Goal

Let an event outside the terminal start a headless Neuron run and get its answer back where the
event came from:

- GitHub: an issue opened or labelled, a pull request opened or updated, or a comment that mentions
  the bot (`@neuron fix the failing test`) on an issue, a pull request or a review thread.
- Slack: a mention of the app in a channel or thread, or a slash command (`/neuron …`).

The run works on a checkout of the repository on a **runner**, in an isolated git worktree, and its
output goes back as a pull request comment (or a new pull request) or a Slack thread reply.

## What exists today

- Headless runs: `neuron prompt "…" --output-format json` (one turn, JSON result) with
  `--permission-mode` and `--allowedTools`. There is no stream-json output yet (`args.rs` accepts
  `text` and `json`).
- `/pr` and `/issue` through `gh` (`rusty-claude-cli/src/github_actions.rs`), and `/commit`.
- Sub-agent worktree isolation (`Agent` with `isolation: "worktree"`,
  `tools/src/agent_worktree.rs`): a worktree under `<NEURON_HOME>/worktrees/…` on a branch
  `neuron/agent/<id>`, removed when unchanged.
- `RemoteTrigger` (`tools/src/lib.rs`) only sends HTTP out; nothing receives events.
- The Zero-X gateway (zero-x.live) routes model traffic and holds the account and quota. It does not
  receive webhooks and should not have to.

## Components

```
GitHub / Slack ──HTTPS──▶ trigger receiver ──queue──▶ runner(s) ──▶ neuron prompt (headless)
                              │ verify signature           │ checkout + worktree
                              │ match rules, dedupe        │ model traffic ──▶ Zero-X gateway
                              ▼                            ▼
                         run records ◀──────── results ── reply poster ──▶ PR comment / Slack reply
```

1. **Trigger receiver**: a small, stateless HTTPS service (`neuron triggers serve`, or the same code
   deployed as a serverless function). It verifies each request, turns it into a **run request**,
   and enqueues it. It answers GitHub within 10 s and Slack within 3 s, so it never runs the agent
   itself.
2. **Queue and run records**: at-least-once delivery, keyed by the event's delivery id so a retried
   webhook does not start a second run. A record holds: source, repository, ref, event id,
   requester, prompt, status, timestamps, the result summary and the reply's id.
3. **Runner**: a machine (self-hosted box, container, or a GitHub Actions job) that takes run
   requests, prepares a checkout, and runs `neuron prompt` there. One run per worktree; a runner may
   run several at once.
4. **Reply poster**: turns the run's JSON result into a comment, a pull request or a Slack message,
   and edits a "working…" placeholder in place.

The receiver and the poster can be one process; the runner is separate so it can live where the
code and the compute are.

### The GitHub Action variant

The simplest deployment needs no receiver at all: a workflow in the user's repository on
`issue_comment`, `issues` and `pull_request` that runs `neuron prompt` on the Actions runner. GitHub
does the verification, the queueing and the checkout; the workflow's `GITHUB_TOKEN` posts the reply.
This should ship first (as `neuron-action`), with the receiver for teams that want Slack, private
runners or many repositories.

## Relation to the Zero-X gateway

- The gateway stays a model gateway. Runners send model traffic to it like any Neuron client, with a
  Zero-X key for the account that pays (a **service key** scoped to "headless runs", stored as a
  runner secret; never the user's interactive login).
- Quota, routing, failover and model choice stay where they are. A run request may name a model;
  `auto` is the default.
- The receiver does not need the gateway, and the gateway does not need to know about GitHub or
  Slack. A later "dispatch from the gateway" (one hosted receiver for all accounts) is possible
  without changing this split: it would be a separate service next to the gateway, sharing only the
  account lookup.

## Authentication and verification

**GitHub**
- Use a GitHub App (not a personal token): per-repository installation, fine-grained permissions
  (contents: write, pull requests: write, issues: write, metadata: read), short-lived installation
  tokens minted per run.
- Verify `X-Hub-Signature-256` (HMAC-SHA256 of the raw body with the webhook secret) in constant
  time before parsing; reject unsigned or mismatched requests with 401.
- Reject deliveries older than 5 minutes when the event carries a timestamp, and dedupe on
  `X-GitHub-Delivery`.
- Only act for allowed senders: repository collaborators with write access by default
  (`author_association` in `OWNER`, `MEMBER`, `COLLABORATOR`), configurable per repository. Comments
  from forks or outside contributors never start a run with write tokens.

**Slack**
- Verify `X-Slack-Signature` (`v0=` HMAC-SHA256 of `v0:{timestamp}:{raw body}` with the signing
  secret), constant time; reject when `X-Slack-Request-Timestamp` is more than 5 minutes off.
- Answer the URL verification challenge; acknowledge events within 3 s, then work.
- Map Slack users to allowed requesters (workspace + user id allowlist, or a channel allowlist), and
  map each channel to a repository (`#shop-dev` → `org/shop`), since a Slack message names no repo.

**Secrets**: the webhook secret, Slack signing secret, App private key and Zero-X service key live
in the receiver's and runner's secret stores, never in the repository, run records or logs. Neuron's
existing credential scrubbing (`plugins/src/subprocess.rs`) keeps them out of the agent's shell.

## The run

1. The runner fetches the repository with an installation token (read-only for the fetch), checks
   out the event's ref (the PR head, or the default branch for an issue), and creates a worktree on
   a new branch `neuron/run/<run id>` — the same layout as sub-agent isolation.
2. It writes the prompt: the event text plus context (issue or PR title and body, the comment, the
   diff for a PR, at most a fixed size), and runs
   `neuron prompt <prompt> --output-format json --permission-mode workspace-write`
   with a tool allowlist from the repository's config (`.neuron/triggers.json`), a wall-clock limit
   (default 20 min) and the iteration limit.
3. Network: the runner allows the gateway and the git host; anything else follows the repository's
   config (deny by default where the sandbox can enforce it — Linux `unshare` today).
4. When the run ends, the runner commits what changed (the `/commit` path, credential files left
   out), pushes the branch with a short-lived token that can write, and hands the result to the
   poster. An unchanged worktree is removed.

**Isolation**: one worktree and one process per run; runs never share a working tree. On shared
runners, each run gets a container (or a fresh VM for untrusted repositories). The runner's home
(`NEURON_HOME`) is per run, so sessions, todos and checkpoints do not leak between runs.

## Output back

- **GitHub**: a placeholder comment ("Working on it…", with a link to the run record) that is edited
  into the final answer: the summary, the files changed, and either a pull request link (for an
  issue: `gh pr create` from the run's branch, "Fixes #N") or a pushed commit to the PR branch (only
  when the PR is from the same repository and the requester asked for a change). Long output is
  cut, with the full log attached as a run artifact. Failures say why, without secrets.
- **Slack**: an immediate thread reply ("On it"), updated with the summary and links (PR, diff
  stat); follow-up messages in the same thread continue the same session (the run record keeps the
  session id, and the runner resumes it).

## Configuration (per repository)

`.neuron/triggers.json`, read from the default branch only (never from the PR being triggered):
triggers on/off per event, the mention keyword, allowed requesters, the tool allowlist and
permission mode, the model, time limits, and whether a run may push to PR branches.

## Failure and abuse handling

- Rate limits per repository and per requester; a queue cap per account.
- A run that fails or times out posts that and keeps its branch for inspection (unless unchanged).
- Prompt injection: issue and comment text is data in the prompt, clearly delimited; the agent's
  tools cannot read the runner's secrets, and the push token is minted only after the run, by the
  runner, not the agent.

## Later

- Stream-json output so the poster can show progress live.
- A hosted receiver next to the gateway for accounts that do not run their own.
- Linear/Jira issue triggers using the same run request shape.
- Scheduled triggers (cron) through the same queue.
