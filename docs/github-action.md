# Neuron GitHub Action

`action.yml` at the repository root is a composite action that installs a pinned Neuron
release, runs `neuron prompt` headless with `--output-format stream-json`
([schema](stream-json.md)), posts the answer as a comment when an issue or pull request
triggered it, and exposes the result as outputs.

Linux and macOS runners only (the installer is `install.sh`).

## Example: answer `@neuron` comments

```yaml
# .github/workflows/neuron.yml
name: Neuron
on:
  issue_comment:
    types: [created]

permissions:
  contents: read
  issues: write
  pull-requests: write

jobs:
  neuron:
    # Only members of the repository may spend the key.
    if: contains(fromJSON('["OWNER","MEMBER","COLLABORATOR"]'), github.event.comment.author_association)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - id: neuron
        uses: RAHUL-DevelopeRR/neuron-v2@v7.0.0-alpha.3
        with:
          neuron-version: 7.0.0-alpha.3
          api-key: ${{ secrets.NEURON_TOKEN }}
          api-key-env: NEURON_TOKEN
          model: auto
          permission-mode: read-only
          trigger: "@neuron"
      - if: steps.neuron.outputs.skipped != 'true'
        run: echo "session ${{ steps.neuron.outputs.session-id }} ended ${{ steps.neuron.outputs.outcome }}"
```

A comment such as `@neuron why does the windows job fail?` runs the agent on the
checked-out repository; the answer comes back as a comment with the session id.

## Example: a scheduled or manual task

```yaml
on:
  workflow_dispatch:
    inputs:
      task: { description: "What to do", required: true }
jobs:
  run:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - id: neuron
        uses: RAHUL-DevelopeRR/neuron-v2@v7.0.0-alpha.3
        with:
          neuron-version: 7.0.0-alpha.3
          prompt: ${{ inputs.task }}
          api-key: ${{ secrets.ANTHROPIC_API_KEY }}
          api-key-env: ANTHROPIC_API_KEY
          permission-mode: workspace-write
          allowed-tools: read_file,glob_search,grep_search,edit_file,write_file
          comment: "false"
      - uses: actions/upload-artifact@v4
        with:
          name: neuron-stream
          path: ${{ steps.neuron.outputs.stream-file }}
```

## Inputs

| input             | default            | meaning |
|-------------------|--------------------|---------|
| `prompt`          | `""`               | the task; with a comment trigger, standing instructions put before the comment's request |
| `trigger`         | `@neuron`          | phrase a comment must contain (comment events); empty runs on every event |
| `model`           | `auto`             | model to run |
| `permission-mode` | `read-only`        | `read-only`, `workspace-write`, `danger-full-access` |
| `allowed-tools`   | `""`               | `--allowedTools` list |
| `api-key`         | (required)         | the credential, from a secret |
| `api-key-env`     | `NEURON_TOKEN`     | variable it goes in (`NEURON_TOKEN`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, ...) |
| `neuron-version`  | (required)         | pinned release, with stream-json support |
| `comment`         | `true`             | post the answer on the triggering issue/PR |
| `github-token`    | `github.token`     | token for the comment |
| `denial-limit`    | `5`                | `NEURON_DENIAL_LIMIT` |

## Outputs

`result` (final answer), `session-id`, `outcome`, `exit-code` (`2` = tool calls were
denied for lack of approval; the answer is still posted), `stream-file` (the event log),
`skipped` (`true` when the comment had no trigger phrase).

## Security

- The credential travels only in an environment variable, is masked with `::add-mask::`
  and never appears on a command line. Neuron strips credential variables from the
  shell commands, hooks and plugins it runs.
- Comment bodies and titles are untrusted: they reach the scripts through environment
  variables and files, never `${{ }}` interpolation into `run:`.
- Nobody can approve tool calls in CI: anything the permission mode does not allow is
  denied, and the run stops after `denial-limit` denials. Start with `read-only` and
  widen `permission-mode` / `allowed-tools` deliberately.
- Guard the workflow (`author_association`, as above) so outside contributors cannot
  spend your key or steer an agent that has write access.
