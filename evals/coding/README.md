# Neuron coding eval

Measures whether Neuron gets coding tasks right with a given model, and whether a
harness change helps: run the same tasks with two Neuron builds (or one build with
different settings) and compare pass rates.

```bash
# Every grader must fail on the untouched task and pass on its reference solution.
python evals/coding/run.py --self-test

# Two builds against one model on NVIDIA NIM (needs NVIDIA_API_KEY).
python evals/coding/run.py --provider nvidia --model nvidia/nemotron-3.5-lightning-30b-a3b \
    --variant baseline=path/to/old/neuron.exe \
    --variant harness=path/to/new/neuron.exe \
    --variant review=path/to/new/neuron.exe,NEURON_REVIEW=1 \
    --repeat 2 --jobs 4
```

`--provider zero-x` uses the gateway (your login or `NEURON_TOKEN`), so
`--model auto` measures what users get. Results go to `results/<time>/`:
`results.jsonl` has one line per run (pass, iterations, tokens, time, served
model, tools used, files changed, the check's output) and `summary.md` the table.

## How a run works

Each run copies the task's `workspace/` into a new git repository, writes project
settings that allow only the task's `allow` commands (plus the read-only project
checks and `python -c` / `node -e`), and runs `neuron prompt` there in
`workspace-write` mode with its own Neuron home and config directory. Other
commands are denied, as in any non-interactive run; `NEURON_DENIAL_LIMIT=25` lets
the model work around them instead of stopping at the default 5. After the agent
finishes, the task's `grader/` is copied in and its `check` command decides.
Changing a file the task lists under `protected` (its tests) fails the run even
when the check passes.

A run the provider ended (rate limit, outage) is started again in a fresh
workspace after a minute (`--retry-infra`, default 2); if it keeps failing it
counts as a provider error and is left out of the pass rate. Fast models on
free tiers hit per-minute limits quickly: NVIDIA NIM allows about 40 requests
a minute, so keep `--jobs` at 1–2 there.

## Tasks

| Task | What it probes |
| --- | --- |
| `py-pagination` | two bugs in one function, from failing visible tests |
| `js-rename` | a rename across modules, imports, a re-export and tests |
| `py-cli-json` | a new CLI flag from an exact output spec, old behaviour unchanged |
| `py-twin-methods` | changing one of two identical method bodies (the first match is the wrong one) |
| `js-long-file` | one fix in the middle of a 530-line file without damaging the rest |
| `py-timeout-units` | a symptom-only report whose cause is in another file |
| `go-reverse` | a Go fix from a failing test |
| `py-format-duration` | a function from an exact spec with edge cases |

## Results so far

2026-10-24, `nvidia/nemotron-3.5-lightning-30b-a3b` on NVIDIA NIM (a small,
fast model of the kind `auto` can route to), two rounds of the eight tasks,
counting only runs the provider did not cut off:

| Build | Passed | Mean time per run |
| --- | --- | --- |
| No harness (feat/tui-parity with harness phase 1 reverted) | 11/14 (79%) | 145 s |
| Harness (phases 1 and 2, reviewer off) | 12/13 (92%) | 129 s |
| Harness with `NEURON_REVIEW=1` | 9/10 (90%) | 295 s |

The harness's gain came mostly from `js-rename` (0/2 without, 2/2 with), where
the model escaped its edits twice and mistyped long paths. The samples are small:
read the gap as a direction, not a measured effect size. The reviewer did not
pass more tasks here and doubled the time per run, so it stays opt-in.

## Adding a task

Create `tasks/<id>/` with `task.json` (`id`, `prompt`, `check`, `allow`,
`grader: {"from": "grader", "to": "<dir>"}`, optional `protected`),
`workspace/` (what the agent starts with), `grader/` (hidden tests) and
`solution/` (files that solve it, for `--self-test` only). Run `--self-test`
before using it.
