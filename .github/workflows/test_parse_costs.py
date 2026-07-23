import unittest

from parse_costs import enforce_cost_budget


class EnforceCostBudgetTests(unittest.TestCase):
    def test_no_regression_within_threshold(self):
        baseline = {"register_project": {"cpu": 1_000_000, "mem": 200_000}}
        pr = {"register_project": {"cpu": 1_040_000, "mem": 200_000}}  # +4%

        self.assertEqual(enforce_cost_budget(baseline, pr, threshold_pct=5.0), [])

    def test_cpu_regression_beyond_threshold_is_flagged(self):
        baseline = {"register_project": {"cpu": 1_000_000, "mem": 200_000}}
        pr = {"register_project": {"cpu": 1_200_000, "mem": 200_000}}  # +20%

        regressions = enforce_cost_budget(baseline, pr, threshold_pct=5.0)

        self.assertEqual(len(regressions), 1)
        self.assertIn("register_project", regressions[0])
        self.assertIn("cpu", regressions[0])

    def test_mem_regression_beyond_threshold_is_flagged(self):
        baseline = {"vote_on_proposal": {"cpu": 500_000, "mem": 100_000}}
        pr = {"vote_on_proposal": {"cpu": 500_000, "mem": 150_000}}  # +50%

        regressions = enforce_cost_budget(baseline, pr, threshold_pct=5.0)

        self.assertEqual(len(regressions), 1)
        self.assertIn("vote_on_proposal", regressions[0])
        self.assertIn("mem", regressions[0])

    def test_improvement_is_not_flagged(self):
        baseline = {"register_project": {"cpu": 1_000_000, "mem": 200_000}}
        pr = {"register_project": {"cpu": 500_000, "mem": 100_000}}

        self.assertEqual(enforce_cost_budget(baseline, pr, threshold_pct=5.0), [])

    def test_new_entrypoint_without_baseline_does_not_crash_or_regress(self):
        baseline = {"register_project": {"cpu": 1_000_000, "mem": 200_000}}
        pr = {
            "register_project": {"cpu": 1_000_000, "mem": 200_000},
            "brand_new_entrypoint": {"cpu": 999_999_999, "mem": 999_999_999},
        }

        self.assertEqual(enforce_cost_budget(baseline, pr, threshold_pct=5.0), [])

    def test_zero_baseline_value_is_skipped_not_crashed(self):
        baseline = {"register_project": {"cpu": 0, "mem": 0}}
        pr = {"register_project": {"cpu": 100, "mem": 100}}

        self.assertEqual(enforce_cost_budget(baseline, pr, threshold_pct=5.0), [])

    def test_multiple_regressions_are_all_reported(self):
        baseline = {
            "register_project": {"cpu": 1_000_000, "mem": 200_000},
            "vote_on_proposal": {"cpu": 500_000, "mem": 100_000},
        }
        pr = {
            "register_project": {"cpu": 1_200_000, "mem": 200_000},
            "vote_on_proposal": {"cpu": 600_000, "mem": 100_000},
        }

        regressions = enforce_cost_budget(baseline, pr, threshold_pct=5.0)

        self.assertEqual(len(regressions), 2)


if __name__ == "__main__":
    unittest.main()
