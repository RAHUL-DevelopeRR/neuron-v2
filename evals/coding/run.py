"""Coding eval for the Neuron agent: runs each task with one or more Neuron builds
("variants") and models, then grades the result with the task's hidden checks.

    python evals/coding/run.py --self-test
    python evals/coding/run.py --provider nvidia --model nvidia/nemotron-3.5-lightning-30b-a3b \
        --variant baseline=path/to/neuron-baseline.exe --variant harness=path/to/neuron.exe

A variant is `NAME=PATH[,KEY=VALUE...]`: the binary to run and extra environment
variables (for example `review=neuron.exe,NEURON_REVIEW=1`). Results go to
`evals/coding/results/<time>/`: `results.jsonl` (one line per run) and `summary.md`.

Each run gets a fresh copy of the task's `workspace/` as its own git repository,
project settings that allow only the task's test commands (the rest of the
permission model is Neuron's default `workspace-write`), and its own Neuron home
and config directories per variant, so nothing from your own setup leaks in. The
grader (`grader/`) is copied in only after the agent has finished, so it cannot
be read or edited, and the task's `check` command decides pass or fail.
"""
import argparse
import concurrent.futures
import datetime
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
TASKS = ROOT / "tasks"
CHECK_TIMEOUT_S = 300
# Always allowed: the project checks Neuron's completion check may propose, and the
# snippet runners models use to try things (no more powerful than the test commands,
# which already run code the model wrote).
DEFAULT_ALLOW = ["bash(python -m py_compile:*)", "bash(go vet:*)", "bash(python -c:*)", "bash(node -e:*)"]
# Denied calls a run may make before Neuron stops it: high enough that a model can
# work around commands the task does not allow; the iteration cap still applies.
DENIAL_LIMIT = "25"
PROVIDERS = {
    # NVIDIA NIM's OpenAI-compatible API with NVIDIA_API_KEY.
    "nvidia": {
        "NEURON_PROVIDER": "openai",
        "OPENAI_BASE_URL": "https://integrate.api.nvidia.com/v1",
        "OPENAI_API_KEY": "$NVIDIA_API_KEY",
    },
    # The Zero-X gateway: NEURON_TOKEN, or the stored login (keeps your Neuron home).
    "zero-x": {"NEURON_PROVIDER": "zero-x"},
}
# Variables that would otherwise pick a provider or change the run.
SCRUBBED_ENV = (
    "NEURON_TOKEN", "NEURON_MODEL", "NEURON_PROVIDER", "NEURON_API_BASE", "NEURON_MAX_ITERATIONS",
    "OPENAI_API_KEY", "OPENAI_BASE_URL", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL", "XAI_API_KEY", "OPENROUTER_API_KEY", "AZURE_OPENAI_API_KEY",
    "AZURE_OPENAI_ENDPOINT", "AZURE_OPENAI_MODEL", "NEURON_COMPLETION_CHECK", "NEURON_AUTO_CHECKS",
    "NEURON_REVIEW", "NEURON_REVIEW_MODEL", "NEURON_EXPERIENCE",
)


def load_tasks(selected):
    tasks = []
    for task_dir in sorted(path for path in TASKS.iterdir() if (path / "task.json").is_file()):
        task = json.loads((task_dir / "task.json").read_text(encoding="utf-8"))
        task["dir"] = task_dir
        if selected and task["id"] not in selected:
            continue
        tasks.append(task)
    missing = set(selected or []) - {task["id"] for task in tasks}
    if missing:
        sys.exit(f"unknown task(s): {', '.join(sorted(missing))}")
    return tasks


def git(workspace, *args):
    return subprocess.run(
        ["git", "-c", "user.name=neuron-eval", "-c", "user.email=eval@localhost", "-c", "core.autocrlf=false",
         "-c", "core.longpaths=true", *args],
        cwd=workspace, capture_output=True, text=True, check=False,
    )


def prepare_workspace(task, workspace):
    shutil.copytree(task["dir"] / "workspace", workspace)
    settings = workspace / ".neuron" / "settings.json"
    settings.parent.mkdir(parents=True, exist_ok=True)
    allow = task.get("allow", []) + DEFAULT_ALLOW
    settings.write_text(json.dumps({"permissions": {"allow": allow}}, indent=2) + "\n", encoding="utf-8")
    for args in (("init", "-q"), ("add", "-A"), ("commit", "-q", "-m", "task start")):
        result = git(workspace, *args)
        if result.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed in {workspace}: {result.stderr.strip()}")


def copy_overlay(source, destination):
    for path in source.rglob("*"):
        if path.is_file():
            target = destination / path.relative_to(source)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)


def run_check(task, workspace):
    grader = task.get("grader")
    if grader:
        copy_overlay(task["dir"] / grader["from"], workspace / grader["to"])
    started = time.monotonic()
    try:
        result = subprocess.run(
            task["check"], cwd=workspace, shell=True, capture_output=True, text=True,
            timeout=CHECK_TIMEOUT_S, encoding="utf-8", errors="replace",
        )
        output = (result.stdout + "\n" + result.stderr).strip()
        return result.returncode == 0, output[-2000:], time.monotonic() - started
    except subprocess.TimeoutExpired:
        return False, f"check timed out after {CHECK_TIMEOUT_S} s", time.monotonic() - started


def kill_tree(process):
    if os.name == "nt":
        subprocess.run(["taskkill", "/T", "/F", "/PID", str(process.pid)], capture_output=True, check=False)
    else:
        process.kill()


def run_neuron(binary, model, prompt, workspace, env, timeout_s):
    command = [binary, "--model", model, "--permission-mode", "workspace-write", "--output-format", "json", "prompt", prompt]
    started = time.monotonic()
    process = subprocess.Popen(
        command, cwd=workspace, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace",
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout_s)
        timed_out = False
    except subprocess.TimeoutExpired:
        kill_tree(process)
        stdout, stderr = process.communicate()
        timed_out = True
    report = None
    for line in reversed(stdout.strip().splitlines()):
        if line.startswith("{"):
            try:
                report = json.loads(line)
                break
            except json.JSONDecodeError:
                continue
    return report, stderr[-2000:], time.monotonic() - started, timed_out, process.returncode


def changed_files(workspace):
    status = git(workspace, "status", "--porcelain", "--untracked-files=all").stdout
    return sorted(line[3:].strip().strip('"') for line in status.splitlines() if line.strip())


def build_env(args, variant, run_home):
    env = {key: value for key, value in os.environ.items() if key not in SCRUBBED_ENV}
    for key, value in PROVIDERS.get(args.provider, {}).items():
        if value.startswith("$"):
            value = os.environ.get(value[1:], "")
            if not value:
                sys.exit(f"--provider {args.provider} needs {PROVIDERS[args.provider][key][1:]} in the environment")
        env[key] = value
    if args.provider == "zero-x" and os.environ.get("NEURON_TOKEN"):
        env["NEURON_TOKEN"] = os.environ["NEURON_TOKEN"]
    if args.provider != "zero-x":
        env["NEURON_HOME"] = str(run_home / "home")
    env["NEURON_CONFIG_HOME"] = str(run_home / "config")
    env.update({
        "NEURON_TRUST_WORKSPACE": "1", "NEURON_NO_BROWSER": "1", "NO_COLOR": "1",
        "NEURON_DENIAL_LIMIT": DENIAL_LIMIT,
    })
    env.update(variant["env"])
    return env


def run_one(args, task, variant, model, repeat, work):
    # Short paths: git on Windows fails past 260 characters.
    group = work / f"{variant['name']}-m{args.model.index(model)}"
    workspace = group / f"{task['id']}-r{repeat}"
    run_home = group / "_neuron"
    prepare_workspace(task, workspace)
    env = build_env(args, variant, run_home)
    report, stderr, seconds, timed_out, exit_code = run_neuron(
        variant["binary"], model, task["prompt"], workspace, env, args.timeout
    )
    changed = changed_files(workspace)
    protected = [path for path in changed if any(path.startswith(prefix) for prefix in task.get("protected", []))]
    check_passed, check_output, _ = run_check(task, workspace)
    report = report or {}
    usage = report.get("usage") or {}
    tool_results = report.get("tool_results") or []
    return {
        "task": task["id"],
        "variant": variant["name"],
        "model": model,
        "repeat": repeat,
        # Editing files the task protects (its tests) fails the run.
        "passed": check_passed and not protected,
        "check_passed": check_passed,
        "seconds": round(seconds, 1),
        "timed_out": timed_out,
        "exit_code": exit_code,
        "outcome": report.get("outcome") or ("timeout" if timed_out else "error"),
        "error": report.get("error"),
        "iterations": report.get("iterations"),
        "served_model": report.get("served_model"),
        "completion_checked": report.get("completion_checked"),
        "input_tokens": usage.get("input_tokens", 0),
        "output_tokens": usage.get("output_tokens", 0),
        "tool_calls": len(report.get("tool_uses") or []),
        "tools": [use.get("name") for use in report.get("tool_uses") or []],
        "tool_errors": sum(1 for result in tool_results if result.get("is_error")),
        "changed_files": changed,
        "touched_protected": protected,
        "message": (report.get("message") or "")[-600:],
        "check_output": check_output[-1200:],
        "stderr": stderr[-800:],
        "workspace": str(workspace),
    }


def summarize(rows):
    lines = ["# Neuron coding eval", ""]
    groups = {}
    for row in rows:
        groups.setdefault((row["variant"], row["model"]), []).append(row)
    lines += ["| variant | model | passed | pass rate | iterations (mean) | tokens in/out (mean) | seconds (mean) | errors |",
              "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    for (variant, model), group in sorted(groups.items()):
        passed = sum(row["passed"] for row in group)
        iterations = [row["iterations"] for row in group if row["iterations"] is not None]
        mean_iterations = f"{statistics.mean(iterations):.1f}" if iterations else "-"
        errors = sum(1 for row in group if row["outcome"] in ("error", "timeout"))
        lines.append(
            f"| {variant} | {model} | {passed}/{len(group)} | {passed / len(group):.0%} | {mean_iterations} | "
            f"{statistics.mean(row['input_tokens'] for row in group):,.0f} / "
            f"{statistics.mean(row['output_tokens'] for row in group):,.0f} | "
            f"{statistics.mean(row['seconds'] for row in group):.0f} | {errors} |"
        )
    lines += ["", "## Per task", ""]
    variants = sorted({(row["variant"], row["model"]) for row in rows})
    header = "| task | " + " | ".join(f"{variant} / {model.split('/')[-1]}" for variant, model in variants) + " |"
    lines += [header, "| --- |" + " --- |" * len(variants)]
    for task in sorted({row["task"] for row in rows}):
        cells = []
        for key in variants:
            group = [row for row in rows if row["task"] == task and (row["variant"], row["model"]) == key]
            cells.append(f"{sum(row['passed'] for row in group)}/{len(group)}" if group else "-")
        lines.append(f"| {task} | " + " | ".join(cells) + " |")
    return "\n".join(lines) + "\n"


def self_test(tasks):
    """Each grader must fail on the untouched workspace and pass on the reference solution."""
    ok = True
    with tempfile.TemporaryDirectory(prefix="neuron-eval-self-test-") as scratch:
        for task in tasks:
            before = Path(scratch) / task["id"] / "before"
            prepare_workspace(task, before)
            failed_before, _, _ = run_check(task, before)
            after = Path(scratch) / task["id"] / "after"
            prepare_workspace(task, after)
            copy_overlay(task["dir"] / "solution", after)
            passed_after, output, seconds = run_check(task, after)
            good = not failed_before and passed_after
            ok &= good
            print(f"{'ok  ' if good else 'FAIL'} {task['id']:22s} unsolved={'fails' if not failed_before else 'PASSES'} "
                  f"solved={'passes' if passed_after else 'FAILS'} ({seconds:.1f} s)")
            if not passed_after:
                print("     " + output.replace("\n", "\n     ")[-1500:])
    return ok


def parse_variant(spec):
    name, _, rest = spec.partition("=")
    parts = rest.split(",")
    binary = parts[0]
    if not name or not binary:
        sys.exit(f"--variant must be NAME=PATH[,KEY=VALUE...]: {spec}")
    if not Path(binary).is_file():
        sys.exit(f"--variant {name}: no such binary: {binary}")
    env = dict(part.split("=", 1) for part in parts[1:] if "=" in part)
    return {"name": name, "binary": binary, "env": env}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--self-test", action="store_true", help="check every grader against its reference solution")
    parser.add_argument("--variant", action="append", default=[], help="NAME=PATH[,KEY=VALUE...]")
    parser.add_argument("--model", action="append", default=[], help="model id (repeatable)")
    parser.add_argument("--provider", choices=sorted(PROVIDERS) + ["env"], default="env",
                        help="how Neuron reaches the model (env: whatever your environment selects)")
    parser.add_argument("--tasks", default="", help="comma-separated task ids (default: all)")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--jobs", type=int, default=1, help="runs in parallel")
    parser.add_argument("--timeout", type=int, default=900, help="seconds per agent run")
    parser.add_argument("--work", help="directory for run workspaces (default: a new temp directory)")
    parser.add_argument("--out", help="results directory (default: evals/coding/results/<time>)")
    args = parser.parse_args()

    tasks = load_tasks([task for task in args.tasks.split(",") if task])
    if args.self_test:
        sys.exit(0 if self_test(tasks) else 1)
    if not args.variant or not args.model:
        parser.error("--variant and --model are required (or use --self-test)")
    variants = [parse_variant(spec) for spec in args.variant]

    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    work = Path(args.work or tempfile.mkdtemp(prefix=f"neuron-eval-{stamp}-"))
    out = Path(args.out or ROOT / "results" / stamp)
    out.mkdir(parents=True, exist_ok=True)
    runs = [(task, variant, model, repeat) for repeat in range(1, args.repeat + 1)
            for model in args.model for variant in variants for task in tasks]
    print(f"{len(runs)} runs; workspaces in {work}; results in {out}", flush=True)

    rows = []
    with open(out / "results.jsonl", "w", encoding="utf-8") as results, \
            concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = [pool.submit(run_one, args, *run, work) for run in runs]
        for future in concurrent.futures.as_completed(futures):
            row = future.result()
            rows.append(row)
            results.write(json.dumps(row) + "\n")
            results.flush()
            print(f"{'PASS' if row['passed'] else 'fail'} {row['task']:20s} {row['variant']:10s} "
                  f"{row['model'].split('/')[-1]:28s} it={row['iterations']} {row['seconds']:.0f}s {row['outcome']}",
                  flush=True)
    summary = summarize(rows)
    (out / "summary.md").write_text(summary, encoding="utf-8")
    print(summary)


if __name__ == "__main__":
    main()
