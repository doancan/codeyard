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

    def test_noise_of_the_same_build_passes(self):
        # Two runs of one build: same spread, different order and a few different draws.
        base = [20 + (i * 7) % 13 for i in range(40)]
        cand = [20 + (i * 5 + 3) % 13 for i in range(40)]
        r = self.run_compare({"t": base}, {"t": cand}, [self.metric()])
        self.assertEqual(r.returncode, 0, r.stdout)

    def test_a_regression_in_the_tail_alone_fails(self):
        # The median does not move, but one key press in four is three times slower: p95 +200%.
        r = self.run_compare({"t": [10] * 19 + [12]}, {"t": [10] * 15 + [30] * 5}, [self.metric()])
        self.assertEqual(r.returncode, 1, r.stdout)

    def test_one_baseline_outlier_does_not_hide_a_memory_regression(self):
        m = self.metric(stat="median", budget=0.05)
        r = self.run_compare({"t": [100, 100, 130]}, {"t": [120, 120, 120]}, [m])
        self.assertEqual(r.returncode, 1, r.stdout)

    def test_one_candidate_outlier_is_not_a_memory_regression(self):
        m = self.metric(stat="median", budget=0.05)
        r = self.run_compare({"t": [100, 101, 100]}, {"t": [100, 130, 101]}, [m])
        self.assertEqual(r.returncode, 0, r.stdout)

    def test_a_blocking_metric_without_baseline_is_reported(self):
        r = self.run_compare({}, {"t": [10, 11]}, [self.metric()])
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("warn: no baseline samples", r.stdout)

    def test_different_machines_are_called_out(self):
        r = self.run_compare({"t": [10]}, {"t": [10]}, [self.metric()])
        self.assertNotIn("different machines", r.stdout)

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
