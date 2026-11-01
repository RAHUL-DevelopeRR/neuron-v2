"""Unit tests for the eval runner's gate, report and provider set-up (no model, no Neuron).

    python -m unittest discover -s evals/coding        # from the repository root
    cd evals/coding && python -m unittest              # or here
"""
import argparse
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run  # noqa: E402


def row(task="t", passed=True, variant="v", model="m", infra=False):
    return {"task": task, "variant": variant, "model": model, "passed": passed, "infra_error": infra}


class GroupStatsTest(unittest.TestCase):
    def test_provider_errors_are_left_out_of_the_rate(self):
        rows = [row(passed=True), row(passed=False), row(passed=False, infra=True)]
        [group] = run.group_stats(rows)
        self.assertEqual(group["runs"], 3)
        self.assertEqual(group["valid"], 2)
        self.assertEqual(group["passed"], 1)
        self.assertEqual(group["provider_errors"], 1)
        self.assertAlmostEqual(group["pass_rate"], 0.5)

    def test_one_group_per_variant_and_model(self):
        rows = [row(variant="a"), row(variant="b", passed=False), row(variant="a", model="n")]
        keys = [(g["variant"], g["model"]) for g in run.group_stats(rows)]
        self.assertEqual(keys, [("a", "m"), ("a", "n"), ("b", "m")])

    def test_all_provider_errors_has_no_rate(self):
        [group] = run.group_stats([row(infra=True), row(infra=True, passed=False)])
        self.assertIsNone(group["pass_rate"])


class GateTest(unittest.TestCase):
    def rows(self, passed, total):
        return [row(task=f"t{i}", passed=i < passed) for i in range(total)]

    def test_exactly_at_the_threshold_passes(self):
        gate = run.evaluate_gate(self.rows(6, 8), 0.75)
        self.assertTrue(gate["passed"])
        self.assertEqual(gate["failures"], [])

    def test_below_the_threshold_fails_and_says_why(self):
        gate = run.evaluate_gate(self.rows(5, 8), 0.75)
        self.assertFalse(gate["passed"])
        self.assertEqual(len(gate["failures"]), 1)
        self.assertIn("62%", gate["failures"][0])
        self.assertIn("5/8", gate["failures"][0])
        self.assertIn("below 75%", gate["failures"][0])

    def test_rounding_does_not_fail_an_exact_match(self):
        # 2/3 against a threshold written as 0.6666666666666666.
        self.assertTrue(run.evaluate_gate(self.rows(2, 3), 2 / 3)["passed"])

    def test_provider_errors_do_not_count_against_the_agent(self):
        rows = self.rows(3, 4) + [row(task="x", passed=False, infra=True)] * 3
        self.assertTrue(run.evaluate_gate(rows, 0.75)["passed"])

    def test_nothing_measured_fails(self):
        self.assertEqual(run.evaluate_gate([], 0.5)["failures"], ["no runs"])
        gate = run.evaluate_gate([row(infra=True)], 0.0)
        self.assertFalse(gate["passed"])
        self.assertIn("provider error", gate["failures"][0])

    def test_every_group_must_pass(self):
        rows = [row(variant="good")] * 4 + [row(variant="bad", passed=False)] * 4
        gate = run.evaluate_gate(rows, 0.5)
        self.assertFalse(gate["passed"])
        self.assertEqual(len(gate["failures"]), 1)
        self.assertTrue(gate["failures"][0].startswith("bad / m"))

    def test_zero_threshold_passes_any_measured_run(self):
        self.assertTrue(run.evaluate_gate(self.rows(0, 4), 0.0)["passed"])

    def test_markdown_names_the_verdict_and_failures(self):
        text = run.gate_markdown(run.evaluate_gate(self.rows(1, 4), 0.75))
        self.assertIn("## Gate: FAILED (minimum pass rate 75%)", text)
        self.assertIn("- v / m: pass rate 25% (1/4) is below 75%", text)
        self.assertIn("## Gate: passed", run.gate_markdown(run.evaluate_gate(self.rows(4, 4), 0.75)))


class GateFileTest(unittest.TestCase):
    def write(self, content):
        handle = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False, encoding="utf-8")
        handle.write(content)
        handle.close()
        self.addCleanup(os.unlink, handle.name)
        return handle.name

    def test_checked_in_gate_file_is_valid(self):
        rate = run.load_gate(run.GATE_FILE)
        self.assertGreater(rate, 0)
        self.assertLessEqual(rate, 1)

    def test_reads_min_pass_rate(self):
        self.assertEqual(run.load_gate(self.write('{"min_pass_rate": 0.6, "note": "x"}')), 0.6)
        self.assertEqual(run.load_gate(self.write('{"min_pass_rate": 1}')), 1.0)

    def test_rejects_bad_files(self):
        for content in ('{"min_pass_rate": 1.5}', '{"min_pass_rate": -0.1}', '{"min_pass_rate": "0.7"}',
                        '{"min_pass_rate": true}', '{}', '[0.7]', 'not json'):
            with self.subTest(content=content), self.assertRaises(ValueError):
                run.load_gate(self.write(content))

    def test_missing_file(self):
        with self.assertRaises(ValueError):
            run.load_gate(Path(tempfile.gettempdir()) / "no-such-neuron-gate.json")


class ReportTest(unittest.TestCase):
    def test_totals_tasks_and_gate(self):
        rows = [row(task="a"), row(task="a", passed=False), row(task="b"),
                row(task="b", passed=False, infra=True)]
        gate = run.evaluate_gate(rows, 0.5)
        report = run.build_report(rows, gate)
        self.assertEqual((report["runs"], report["valid"], report["passed"], report["provider_errors"]),
                         (4, 3, 2, 1))
        self.assertAlmostEqual(report["pass_rate"], 2 / 3)
        self.assertEqual(report["tasks"], {"a": {"passed": 1, "valid": 2}, "b": {"passed": 1, "valid": 1}})
        self.assertTrue(report["gate"]["passed"])
        json.dumps(report)  # serialisable as written by --json-out

    def test_empty_run(self):
        report = run.build_report([])
        self.assertIsNone(report["pass_rate"])
        self.assertIsNone(report["gate"])


class ProviderEnvTest(unittest.TestCase):
    def args(self, **overrides):
        values = {"provider": "mock", "base_url": "http://127.0.0.1:9"}
        values.update(overrides)
        return argparse.Namespace(**values)

    def test_mock_provider_points_at_the_base_url_with_a_fake_key(self):
        variant = {"name": "v", "binary": "neuron", "env": {"X": "1"}}
        with mock.patch.dict(os.environ, {"NEURON_TOKEN": "real-token", "ANTHROPIC_BASE_URL": "https://x"}):
            env = run.build_env(self.args(), variant, Path("home"))
        self.assertEqual(env["ANTHROPIC_BASE_URL"], "http://127.0.0.1:9")
        self.assertEqual(env["ANTHROPIC_API_KEY"], "mock-eval-key")
        self.assertNotIn("NEURON_TOKEN", env)
        self.assertEqual(env["NEURON_HOME"], str(Path("home") / "home"))
        self.assertEqual(env["X"], "1")

    def test_mock_provider_reads_the_url_from_the_environment(self):
        variant = {"name": "v", "binary": "neuron", "env": {}}
        with mock.patch.dict(os.environ, {"NEURON_EVAL_MOCK_BASE_URL": "http://127.0.0.1:7"}):
            env = run.build_env(self.args(base_url=None), variant, Path("home"))
        self.assertEqual(env["ANTHROPIC_BASE_URL"], "http://127.0.0.1:7")

    def test_mock_provider_without_a_url_stops(self):
        variant = {"name": "v", "binary": "neuron", "env": {}}
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("NEURON_EVAL_MOCK_BASE_URL", None)
            with self.assertRaises(SystemExit):
                run.build_env(self.args(base_url=None), variant, Path("home"))

    def test_zero_x_passes_the_token_through(self):
        variant = {"name": "v", "binary": "neuron", "env": {}}
        with mock.patch.dict(os.environ, {"NEURON_TOKEN": "t0ken"}):
            env = run.build_env(self.args(provider="zero-x", base_url=None), variant, Path("home"))
        self.assertEqual(env["NEURON_TOKEN"], "t0ken")
        self.assertEqual(env["NEURON_PROVIDER"], "zero-x")


class TasksTest(unittest.TestCase):
    def test_default_and_mock_task_sets(self):
        self.assertEqual(len(run.load_tasks([])), 8)
        self.assertEqual([task["id"] for task in run.load_tasks([], run.MOCK_TASKS)], ["mock-write-file"])

    def test_mock_tasks_name_a_scenario(self):
        for task in run.load_tasks([], run.MOCK_TASKS):
            self.assertTrue(task["prompt"].startswith("PARITY_SCENARIO:"), task["id"])


class CliTest(unittest.TestCase):
    def cli(self, *args):
        return subprocess.run([sys.executable, str(Path(run.__file__)), *args],
                              capture_output=True, text=True, check=False)

    def test_bad_threshold_is_a_usage_error(self):
        result = self.cli("--min-pass-rate", "1.5", "--self-test")
        self.assertEqual(result.returncode, 2)
        self.assertIn("from 0 to 1", result.stderr)

    def test_mock_self_test_passes(self):
        result = self.cli("--self-test", "--tasks-dir", str(run.MOCK_TASKS))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("ok   mock-write-file", result.stdout)


if __name__ == "__main__":
    unittest.main()
