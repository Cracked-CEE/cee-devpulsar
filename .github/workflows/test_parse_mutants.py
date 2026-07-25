import unittest

from parse_mutants import generate_markdown, summarize


def outcome(summary, name="contracts/tansu/src/contract_dao.rs:1:1: replace x with y"):
    return {"summary": summary, "scenario": {"Mutant": {"name": name}}}


class SummarizeTests(unittest.TestCase):
    def test_score_is_caught_over_caught_plus_missed(self):
        outcomes = {
            "total_mutants": 4,
            "caught": 3,
            "missed": 1,
            "timeout": 0,
            "unviable": 0,
            "outcomes": [
                outcome("CaughtMutant"),
                outcome("CaughtMutant"),
                outcome("CaughtMutant"),
                outcome("MissedMutant", "contracts/tansu/src/contract_dao.rs:5:5: x"),
            ],
        }

        summary = summarize(outcomes)

        self.assertEqual(summary["score_pct"], 75.0)
        self.assertEqual(
            summary["missed_mutants"], ["contracts/tansu/src/contract_dao.rs:5:5: x"]
        )

    def test_unviable_mutants_excluded_from_score(self):
        # 1 caught, 0 missed, 3 unviable -> score should be 100%, not 25%.
        outcomes = {
            "total_mutants": 4,
            "caught": 1,
            "missed": 0,
            "timeout": 0,
            "unviable": 3,
            "outcomes": [outcome("CaughtMutant")],
        }

        summary = summarize(outcomes)

        self.assertEqual(summary["score_pct"], 100.0)

    def test_timeouts_reported_separately_and_excluded_from_score(self):
        outcomes = {
            "total_mutants": 3,
            "caught": 1,
            "missed": 1,
            "timeout": 1,
            "unviable": 0,
            "outcomes": [
                outcome("CaughtMutant"),
                outcome("MissedMutant"),
                outcome("Timeout", "contracts/tansu/src/contract_dao.rs:9:9: slow"),
            ],
        }

        summary = summarize(outcomes)

        self.assertEqual(summary["score_pct"], 50.0)
        self.assertEqual(
            summary["timeout_mutants"],
            ["contracts/tansu/src/contract_dao.rs:9:9: slow"],
        )

    def test_no_scored_mutants_yields_none_not_crash(self):
        outcomes = {
            "total_mutants": 1,
            "caught": 0,
            "missed": 0,
            "timeout": 0,
            "unviable": 1,
            "outcomes": [],
        }

        summary = summarize(outcomes)

        self.assertIsNone(summary["score_pct"])


class GenerateMarkdownTests(unittest.TestCase):
    def test_reports_na_score_without_crashing(self):
        summary = summarize(
            {
                "total_mutants": 1,
                "caught": 0,
                "missed": 0,
                "timeout": 0,
                "unviable": 1,
                "outcomes": [],
            }
        )

        markdown = generate_markdown(summary)

        self.assertIn("N/A", markdown)

    def test_lists_missed_mutant_names(self):
        summary = summarize(
            {
                "total_mutants": 1,
                "caught": 0,
                "missed": 1,
                "timeout": 0,
                "unviable": 0,
                "outcomes": [
                    outcome(
                        "MissedMutant",
                        "contracts/tansu/src/contract_dao.rs:42:1: replace > with >=",
                    )
                ],
            }
        )

        markdown = generate_markdown(summary)

        self.assertIn(
            "contracts/tansu/src/contract_dao.rs:42:1: replace > with >=", markdown
        )


if __name__ == "__main__":
    unittest.main()
