"""scripts/perf/compare.py: budgets, the noise floor, ceilings and missing samples."""
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

COMPARE = pathlib.Path(__file__).with_name("perf") / "compare.py"


class CompareTests(unittest.TestCase):
    def run_compare(self, base, cand, metrics):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            for name, runs in (("base", base), ("cand", cand)):
                (d / f"{name}.json").write_text(json.dumps({"version": name, "commit": "0" * 40, "runs": runs}))
            (d / "metrics.json").write_text(json.dumps({"metrics": metrics}))
            return subprocess.run(
                [sys.executable, "-I", str(COMPARE), str(d / "base.json"), str(d / "cand.json"), "--catalogue", str(d / "metrics.json")],
                capture_output=True, text=True)

    def metric(self, **kw):
        m = {"id": "t", "stat": "p95", "budget": 0.10, "ceiling": None, "gate": "block"}
        m.update(kw)
        return m

    def test_same_samples_pass(self):
        r = self.run_compare({"t": [10, 11, 12, 13]}, {"t": [10, 11, 12, 13]}, [self.metric()])
        self.assertEqual(r.returncode, 0, r.stdout)

    def test_clear_regression_fails(self):
        r = self.run_compare({"t": [10, 10, 11, 11]}, {"t": [15, 15, 16, 16]}, [self.metric()])
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("regressed more than 10%", r.stdout)

    def test_shift_inside_the_baseline_spread_passes(self):
        # p95 is 20% worse, but the candidate's median (12) is not above the baseline's p95 (20): noise.
        r = self.run_compare({"t": [10, 11, 12, 20]}, {"t": [10, 11, 12, 24]}, [self.metric()])
        self.assertEqual(r.returncode, 0, r.stdout)

    def test_warn_metric_never_fails(self):
        r = self.run_compare({"t": [10, 10]}, {"t": [30, 30]}, [self.metric(gate="warn")])
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("warn:", r.stdout)

    def test_ceiling_fails_without_baseline(self):
        r = self.run_compare({}, {"t": [30, 30]}, [self.metric(ceiling=20)])
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("over ceiling", r.stdout)

    def test_missing_samples_of_a_blocking_metric_fail(self):
        r = self.run_compare({"t": [10]}, {}, [self.metric()])
        self.assertEqual(r.returncode, 1, r.stdout)

    def test_improvement_passes(self):
        r = self.run_compare({"t": [20, 20, 21]}, {"t": [10, 10, 11]}, [self.metric(stat="median")])
        self.assertEqual(r.returncode, 0, r.stdout)


if __name__ == "__main__":
    unittest.main()
