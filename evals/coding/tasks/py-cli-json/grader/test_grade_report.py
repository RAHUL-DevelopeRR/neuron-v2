import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.getcwd()


def run(*args):
    return subprocess.run(
        [sys.executable, os.path.join(HERE, "report.py"), *args],
        capture_output=True,
        text=True,
        timeout=60,
    )


def write_csv(rows):
    handle = tempfile.NamedTemporaryFile("w", suffix=".csv", delete=False, encoding="utf-8", newline="")
    handle.write("id,customer,amount\n")
    for row in rows:
        handle.write(",".join(row) + "\n")
    handle.close()
    return handle.name


class GradeReport(unittest.TestCase):
    def setUp(self):
        self.orders = write_csv(
            [("A-100", "Lena", "19.99"), ("A-101", "Omar", "250.00"), ("A-102", "Lena", "5.50"), ("A-103", "Chen", "99.95")]
        )

    def test_text_output_is_unchanged(self):
        result = run(self.orders)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "4 orders, total 375.44\ncustomers: Chen, Lena, Omar\n")

    def check_json(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        data = json.loads(result.stdout)
        self.assertEqual(set(data), {"orders", "total", "customers", "largest_order"})
        self.assertEqual(data["orders"], 4)
        self.assertEqual(data["total"], 375.44)
        self.assertEqual(data["customers"], ["Chen", "Lena", "Omar"])
        self.assertEqual(data["largest_order"], {"id": "A-101", "amount": 250.0})

    def test_json_after_the_path(self):
        self.check_json(run(self.orders, "--json"))

    def test_json_before_the_path(self):
        self.check_json(run("--json", self.orders))

    def test_json_with_no_orders(self):
        empty = write_csv([])
        result = run("--json", empty)
        self.assertEqual(result.returncode, 0, result.stderr)
        data = json.loads(result.stdout)
        self.assertEqual(data, {"orders": 0, "total": 0, "customers": [], "largest_order": None})

    def test_wrong_usage_exits_2(self):
        for args in [(), ("--json",), (self.orders, self.orders)]:
            result = run(*args)
            self.assertEqual(result.returncode, 2, args)
            self.assertIn("usage", result.stderr.lower(), args)


if __name__ == "__main__":
    unittest.main()
