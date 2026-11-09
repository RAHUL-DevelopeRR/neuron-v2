# Hooks

Hooks are shell commands Neuron runs at fixed points of a session: around tool calls, when
a session starts, when you send a prompt, when the agent is about to finish, before
compaction and when a sub-agent finishes. They are deterministic: they run every time,
whatever the model decides.

## Where hooks come from

| Source | Where | Loads when |
| --- | --- | --- |
| user | `~/.neuron/settings.json` | always |
| project | `.neuron/settings.json`, `.neuron.json` (and the `.claw` equivalents) | the workspace is trusted |
| local | `.neuron/settings.local.json` | the workspace is trusted |
| plugin | an enabled plugin's `plugin.json` `hooks` | the plugin is enabled |

An untrusted workspace loads none of its own hooks. For each event, a later settings file's
list replaces an earlier one's (user, then project, then local); plugin hooks are added
after them. `/hooks` lists what is configured now, by event, with each hook's matcher and
source, and whether the workspace is trusted. `/hooks Stop` lists one event.

## Configuring

```json
{
  "hooks": {
    "PreToolUse": [
      "./scripts/audit.sh",
      { "matcher": "bash", "command": "./scripts/guard-bash.sh" },
      { "matcher": "write_file|edit_file", "hooks": [{ "type": "command", "command": "./scripts/lint-on-write.sh" }] }
    ],
    "SessionStart": [{ "matcher": "startup|resume", "command": "git status --short" }],
    "UserPromptSubmit": ["./scripts/prompt-policy.sh"],
    "Stop": ["./scripts/require-green-tests.sh"],
    "PreCompact": ["./scripts/save-notes.sh"],
    "SubagentStop": ["./scripts/check-agent-report.sh"]
  }
}
```

Each entry is a command string (runs for everything), an object with `command` and an
optional `matcher`, or an object with `matcher` and a `hooks` array of commands (strings
or `{"type": "command", "command": ...}`). The plain list of strings from earlier
versions still works.

Plugins declare the same events in `plugin.json` as lists of command strings (paths are
relative to the plugin's folder); plugin hooks have no matcher.

### Matchers

| Event | The matcher is compared with |
| --- | --- |
| `PreToolUse`, `PostToolUse`, `PostToolUseFailure` | the tool name (`bash`, `read_file`, `mcp__server__tool`, ...) |
| `SessionStart` | the session source: `startup`, `resume`, `clear` or `compact` |
| `PreCompact` | the trigger: `manual` (`/compact`) or `auto` |
| `UserPromptSubmit`, `Stop`, `SubagentStop` | nothing: every hook runs |

No matcher, `""` or `*` matches everything. A matcher of names and `|` (`write_file|edit_file`)
matches those names exactly, ignoring case. Anything else is a regular expression that must
match the whole name (`mcp__.*`). An invalid regular expression is a settings error.

## How a hook runs

The command runs in the platform shell (`sh -lc` on Unix, `cmd /d /s /c` on Windows) in
Neuron's working directory, with credential variables removed from its environment. It gets
a JSON object on stdin and `HOOK_EVENT` in the environment. Hooks of one event run in order;
the first that blocks or fails stops the rest. Interrupting the turn (Esc / Ctrl-C) stops a
running hook.

Every payload has `hook_event_name`, `cwd` and `session_id`. Per event:

| Event | Fields |
| --- | --- |
| `PreToolUse` | `tool_name`, `tool_input` (parsed), `tool_input_json` |
| `PostToolUse` | the above, `tool_output`, `tool_result_is_error` |
| `PostToolUseFailure` | the above with `tool_error` |
| `SessionStart` | `source` |
| `UserPromptSubmit` | `prompt` |
| `Stop` | `stop_hook_active`, `last_assistant_message` |
| `PreCompact` | `trigger` |
| `SubagentStop` | `agent_id`, `agent_type`, `status`, `result`, `stop_hook_active` |

Tool hooks also get `HOOK_TOOL_NAME`, `HOOK_TOOL_INPUT`, `HOOK_TOOL_IS_ERROR` and
`HOOK_TOOL_OUTPUT` in the environment.

### Exit codes and output

| Exit | Meaning |
| --- | --- |
| 0 | Allowed. Stdout is the hook's message (see below). |
| 2 | Blocked. Stdout is the reason. |
| other | The hook failed: a warning with its stderr is shown to you; for `PreToolUse` the tool call is denied. |

Stderr never reaches the model (commands and stderr can hold secrets); a failing hook's stderr is shown to you.

Stdout may be plain text or a JSON object:

- `"decision": "block"` or `"continue": false` blocks, like exit 2; `"reason"` says why.
- `"systemMessage"` and `hookSpecificOutput.additionalContext` are messages.
- `PreToolUse` only: `hookSpecificOutput.permissionDecision` (`allow`, `deny`, `ask`) with
  `permissionDecisionReason`, and `hookSpecificOutput.updatedInput` to rewrite the call's input.

What a message or a block does depends on the event:

| Event | Message (exit 0) | Block (exit 2 / `block`) |
| --- | --- | --- |
| `PreToolUse` | added to the tool result | the call does not run; the reason is its result |
| `PostToolUse`, `PostToolUseFailure` | added to the tool result | the result is marked as an error |
| `SessionStart` | added as context to your next prompt | nothing (a session cannot be refused) |
| `UserPromptSubmit` | added as context under your prompt | the prompt is not sent and the reason is shown |
| `Stop` | nothing | the agent keeps working, told the reason |
| `PreCompact` | nothing | nothing (compaction goes ahead) |
| `SubagentStop` | nothing | the sub-agent keeps working, told the reason |

Context from hooks follows your prompt under a `[Context from hooks]` heading.

`Stop` and `SubagentStop` can keep a turn going at most 3 times; after that the turn ends
whatever the hooks say. They receive `stop_hook_active: true` once they have kept it going,
so a hook can let it end.

Sub-agents run the main agent's tool hooks for their own tool calls; `SessionStart`,
`UserPromptSubmit`, `Stop` and `PreCompact` run for the main agent only.

## In the full-screen UI

A UI that declares the `hook_progress` capability receives `hook_started` and
`hook_finished` events (`event`, `matcher`, `command` shortened with secret-looking values
hidden, `tool_use_id`; `outcome` `ok`/`blocked`/`error`/`cancelled`, `duration_ms` and the
start of the hook's `message` when finished). The terminal UI shows the running hook on the
status line and a chat line for a hook that blocked or failed. The line REPL shows hook
progress with `NEURON_DEBUG` set.
