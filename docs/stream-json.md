# `--output-format stream-json`

`neuron prompt --output-format stream-json "..."` prints the run as newline-delimited
JSON on stdout **while it happens**: one JSON object per line, flushed as soon as it is
written. It is meant for CI, the [GitHub Action](github-action.md), cloud runners and any
program that wants to show progress of a headless run.

```sh
neuron prompt --output-format stream-json --permission-mode read-only \
  "Summarise the failing test" < /dev/null | jq -c 'select(.type != "text_delta")'
```

`--output-format json` is unchanged: one object at the end. `stream-json` ends with a
`result` line carrying exactly the fields of that object, so a script can switch formats
and read the last line.

## Lines

Every line has a `type`. A run prints, in order:

1. one `init` line;
2. any number of **engine protocol events** (the same `type`-tagged JSON the full-screen
   UI reads, from `neuron-tui/src/protocol.rs`, protocol version in `init.protocol`);
3. one `result` line, also when the run fails.

Unknown `type`s and unknown fields must be ignored: new events and fields are added
without a schema bump. Removing or renaming a field bumps `schema`.

### `init`

| field             | type     | meaning                                                     |
|-------------------|----------|-------------------------------------------------------------|
| `schema`          | integer  | this document's version (`1`)                               |
| `protocol`        | integer  | engine protocol version of the events that follow           |
| `version`         | string   | Neuron version                                              |
| `session_id`      | string   | the session; `neuron --resume <id> prompt ...` continues it |
| `model`           | string   | the model asked for (`auto` picks per request)              |
| `cwd`             | string   | working directory of the run                                |
| `permission_mode` | string   | `read-only`, `workspace-write`, `danger-full-access`        |
| `tools`           | string[] | built-in tools the run may call (after `--allowedTools`); MCP and plugin tools are not listed |

### Events

| `type`               | fields                                                                 |
|----------------------|------------------------------------------------------------------------|
| `text_delta`         | `text`: a streamed piece of the assistant's Markdown                   |
| `text_block_end`     | the current text block ended                                           |
| `reasoning_started`  | `chars` (or null), `redacted`; reasoning text itself is not streamed   |
| `tool_input_started` | `id`, `name`: the model started writing a tool call                    |
| `tool_call_started`  | `id`, `name`, `input` (JSON text), `cut_off`                           |
| `tool_result`        | `id`, `name`, `output`, `is_error`, `truncated`, `output_chars`        |
| `request_usage`      | `usage`: `{input, output, cache_read, cache_write}` of one model request |
| `todos`              | `items`: the agent's task list                                         |
| `subagent_started`   | `id`, `description`, `subagent_type`                                   |
| `subagent_finished`  | `id`, `status`, `is_error`, `result`, `note`                           |
| `notice`             | `message`, optional `kind` (`refusal`)                                 |
| `error`              | `message`, optional `detail`                                           |

`tool_result.output` is cut to 4,000 characters (`truncated: true`, `output_chars` is
the full length). The full output is in the session file and in `result.tool_results`.
Outputs over the context cap (40,000 characters) are also kept whole in the session's
artifact store (`/artifacts`); the model then sees a preview with the artifact id.

Left out on purpose: `output_progress`, `tool_input_progress` (high rate) and reasoning
text. Other protocol events may appear (for example `changes`) and should be ignored
when not needed.

### `result`

`type: "result"`, `schema`, `is_error` (true when the run failed), and the fields of
`--output-format json`:

| field                  | meaning                                                         |
|------------------------|-----------------------------------------------------------------|
| `message`              | the final assistant text                                        |
| `model`, `served_model`| the model asked for, the model that answered last              |
| `session_id`           | as in `init`                                                    |
| `iterations`           | model requests in the turn                                      |
| `outcome`, `stop_reason`, `refusal` | how the turn ended                                 |
| `tool_uses`, `tool_results` | every tool call and its full result                        |
| `usage`                | `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens` |
| `estimated_cost`, `estimated_cost_known` | cost estimate, and whether a price list knew the model |
| `auto_compaction`, `prompt_cache_events`, `completion_checked` | as in `json` |

A failed run's `result` has `is_error: true` and the failure fields of `json` mode
(`error`, plus what the turn did before it), and the process exits non-zero. Exit code `2` means tool calls
were denied because nobody could approve them (see `USAGE.md`).

## stderr

Notices (retries, piped-stdin notes, denials) stay on **stderr** as JSON lines
(`{"type":"notice",...}`), exactly as in `json` mode. Errors before the run starts
(bad flags, no credentials) are a single JSON error object on stderr.

## Input

`--input-format stream-json` (a multi-turn conversation fed as JSON lines on stdin) is
not implemented: the bidirectional JSON channel already exists as the engine protocol
(`neuron` started by the full-screen UI over stdio), and a second input schema would
duplicate it. Pipe text on stdin to add context to a single prompt.
