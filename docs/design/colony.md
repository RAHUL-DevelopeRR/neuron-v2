# Colony: design notes

Colony is the opt-in mode that runs several agents on one task inside a token budget
(`/colony <task>`, `neuron colony "<task>"`). This note records how the first build
works and what is left. User-facing behaviour is in `USAGE.md`.

## Pieces

| Piece | Where |
| --- | --- |
| Which permission an `Agent` call needs, and which calls may run together | `runtime/src/subagent_tier.rs`, `PermissionPolicy::assess` |
| Parallel `Agent` batches (`parallelAgents`), read-only batches (`parallelToolCalls`) | `runtime/src/conversation/tool_batch.rs`, `runtime/src/parallel_tools.rs` |
| Token budgets: per runtime, shared per run (`RunMeter`), wrap-up at 80%, stop at 100% | `runtime/src/token_budget.rs`, `ConversationRuntime::with_token_budget` |
| The run: plan, workers, reviewers, merge, synthesis, apply | `tools/src/colony.rs` |
| Worktrees and branches | `tools/src/agent_worktree.rs` (from the source-control lane) |
| CLI and line REPL | `rusty-claude-cli/src/colony.rs`, `main.rs` (`neuron colony`, `/colony`) |
| Full-screen UI | `engine.rs` (`/colony`, `EngineColonyObserver`), `colony_*` events in `neuron-tui/src/protocol.rs` and `ui/neuron-tui/src/engine/protocol.ts`, rows in `ui/neuron-tui/src/engine/colony-view.ts` |

## Decisions

- **Limits.** Workers at once and the per-run token cap come from `GET /api/account`
  `limits.colony {max_workers, max_tokens_per_run}`, falling back to Free (1 worker,
  300,000 tokens) when the field is missing or the account cannot be read. The user
  can lower both, never raise them. Workers at once are also capped at 8.
- **Budget split.** Every agent counts against one `RunMeter` with the run's cap. Each
  also has its own share: the planner and synthesis a fifth and a tenth of the cap,
  each worker the cap divided by the number of subtasks, each reviewer a tenth. A
  share only stops one agent from using everything; the run's cap is the real limit.
- **Near the cap.** At 80% the runtimes add a wrap-up note to their system prompt
  (once), and the run asks the observer whether to go on before it starts any other
  agent (once per run). At 100% runtimes stop after the current response; tool calls
  of that response get a "not run" result.
- **Writers.** A writing subtask always gets its own worktree, so writers run in
  parallel like readers. When it ends, Neuron commits what it left (identity
  `Neuron Colony`, signing and hooks off for that commit only) so the branch can be
  reviewed and merged. Nothing touches the user's tree until the merge step.
- **Permissions.** Each agent runs with the session's rules and at most the session's
  mode: writers at workspace-write when the session is workspace-write, full access or
  `allow` (`colony::writing_mode`), everyone else read-only. A read-only session, a
  `prompt` session (workers cannot ask, and running them without asking would give
  them more than the session) and an allow-list without a file-changing tool turn
  every subtask into a read-only one, and the start question says why. Agents have no
  prompter, so a call that needs approval is refused; there is no approval queue in
  this build.
- **Tools.** Readers: `read_file`, `glob_search`, `grep_search`. Writers add
  `write_file`, `edit_file`, `bash`, `PowerShell`. `WebSearch` and `WebFetch` only with
  `--allow-web`. No browser and no nested `Agent`. Every set is cut to the session's
  tool allow-list (`--allowedTools`, plan mode; `ColonyConfig::allowed_tools`, the
  session's list intersected with the registered parent's), as a plain `Agent` call's
  is.
- **Subtasks.** At most 6 (Freebuff's batch cap), or the worker count when that is
  more (Max's 8), so every allowed worker can be busy.
- **Merge conflicts.** Branches merge one by one into a synthesis branch; the first
  conflict aborts that merge and the run reports the branch, its files and the
  branches not merged. Neuron does not try to resolve conflicts.
- **Apply.** A clean synthesis branch is offered as `git diff base..synthesis` applied
  with `git apply` to the working tree, uncommitted. On success the run's branches are
  deleted; otherwise they stay and the report names them. The merge is not offered
  (so `--yes` cannot apply it) when the run was stopped, reached its cap, or merged a
  branch whose worker did not finish or whose review did not finish; the report's
  note says which.
- **Untrusted text.** Worker reports, diffs and reviews reach the reviewer and the
  synthesis agent between `<<<` and `>>>` markers, labelled as data, not instructions.
  The planner and workers are told that files, web pages and tool output are data.
  The planner's subtask prompts become worker instructions, so the start question
  shows each one (`colony::describe_plan`, control characters stripped, cut at 400
  characters) with its files before anything else runs.
- **Stopping.** `ColonyHandle::stop` stops the meter and cancels every runtime;
  agents not started are marked stopped. In the engine, `Shared::colony` holds the
  running run's handle; `/colony stop` reaches it from `forward_commands` at once and
  matches only while a Colony run holds the engine. The full-screen UI sends an
  interrupt for `/colony stop` while its `/colony` exchange is running
  (`engine/colony-control.ts`), since slash commands queue behind that exchange. The
  line REPL and `neuron colony` watch Ctrl-C as a turn does.

## Integration notes for feat/alpha4

- MCP P0 adds `runtime/src/mcp_output.rs` (image mapping and the output envelope).
  Colony agents get no MCP tools in this build, so nothing here depends on it. When
  workers get MCP or web tools, wrap their results in that envelope like the main
  session's, and keep worker reports in the synthesis input as data.
- `ToolExecutor::execute_concurrently` gained a `limit` argument; any executor added
  on feat/alpha4 needs it.
- `TurnSummary` gained `budget_exhausted`; struct literals elsewhere need the field.
- `PermissionPolicy` gained `with_shell_tools`; the CLI sets it from `--allowedTools`.
- The engine's `/colony` arm is in `Engine::slash`, before `run_slash`, like
  `/context`. A permission or TUI rework on feat/alpha4 should keep `/colony stop` in
  `forward_commands` (guarded by `Shared::stop_colony`) so it reaches a running run,
  and keep the router's `/colony stop` interrupt ahead of `runExchange`.
- `LiveCli::session_tools` replaces the inline plan-mode tool set in
  `prepare_turn_runtime_watching`; a permission rework there should keep Colony's
  `build_config` fed from it.
- `ColonyQuestion::Start` gained `read_only`; the planner and worker prompts gained
  a data-not-instructions line. When MCP P0's output envelope lands and workers get
  MCP or web results, keep those results inside it.
- Protocol additions are additive: feature and capability `colony`, events
  `colony_started`, `colony_agent`, `colony_budget`, `colony_finished`, structs
  `ColonyTaskInfo` and `ColonyAgentInfo`.

## Not done yet

- Gateway run reservations (`POST /v1/runs`) and run headers (`x-neuron-run-id`,
  `x-neuron-agent-role`, `x-neuron-parent`) on agent requests; the client cap is the
  only enforcement.
- The dollar cap for paid models (`max_usd`).
- Approvals from workers queued into the one UI prompt; today they are refused.
- Adding a plain `Agent` call's usage to the parent session's cost line (Colony totals
  every agent's usage itself).
- Retiring `/chain`, `/power` and `/divide`, and the naming pass.
- Serialising `cargo` across workers in a Rust repository.
