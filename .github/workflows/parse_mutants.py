"""Summarize a cargo-mutants `outcomes.json` report as CI job-summary markdown.

cargo-mutants (https://mutants.rs/) classifies each generated mutant as one of:
  - caught:   a test failed -> the suite detected the injected fault (good).
  - missed:   every test still passed -> a gap in test coverage (actionable).
  - timeout:  the test run was killed for running too long; inconclusive, but
              worth investigating (may indicate an infinite loop was introduced,
              or that the timeout heuristic needs tuning).
  - unviable: the mutated code didn't even compile; not a coverage signal.

The mutation score is caught / (caught + missed) -- i.e. the fraction of
mutants that were viable and did not time out for which the test suite
actually detected the injected fault. Unviable mutants carry no test-coverage
signal and are excluded; timeouts are reported separately since "the suite
hung" is a different failure mode from "the suite silently accepted the bug".
"""

import argparse
import json
import sys
from pathlib import Path


def summarize(outcomes: dict) -> dict:
    """Reduce a cargo-mutants outcomes.json dict to the counts we report on."""
    caught = outcomes.get("caught", 0)
    missed = outcomes.get("missed", 0)
    timeout = outcomes.get("timeout", 0)
    unviable = outcomes.get("unviable", 0)
    total = outcomes.get("total_mutants", caught + missed + timeout + unviable)

    scored = caught + missed
    score_pct = (caught / scored * 100) if scored else None

    missed_mutants = [
        scenario_name(o)
        for o in outcomes.get("outcomes", [])
        if o.get("summary") == "MissedMutant"
    ]
    timeout_mutants = [
        scenario_name(o)
        for o in outcomes.get("outcomes", [])
        if o.get("summary") == "Timeout"
    ]

    return {
        "total": total,
        "caught": caught,
        "missed": missed,
        "timeout": timeout,
        "unviable": unviable,
        "score_pct": score_pct,
        "missed_mutants": missed_mutants,
        "timeout_mutants": timeout_mutants,
    }


def scenario_name(outcome: dict) -> str:
    scenario = outcome.get("scenario", {})
    if isinstance(scenario, dict) and "Mutant" in scenario:
        return scenario["Mutant"].get("name", "<unknown mutant>")
    return str(scenario)


def generate_markdown(summary: dict) -> str:
    lines = ["## \U0001f9ec Mutation Testing Summary (contract_dao.rs)", ""]

    score_pct = summary["score_pct"]
    score_str = f"{score_pct:.1f}%" if score_pct is not None else "N/A"

    lines.extend(
        [
            "| Metric | Count |",
            "|--------|-------|",
            f"| Total mutants generated | {summary['total']} |",
            f"| Caught (test suite detected the fault) | {summary['caught']} |",
            f"| **Missed (undetected — coverage gap)** | **{summary['missed']}** |",
            f"| Timeout (inconclusive) | {summary['timeout']} |",
            f"| Unviable (didn't compile, no signal) | {summary['unviable']} |",
            f"| **Mutation score** (caught / (caught + missed)) | **{score_str}** |",
            "",
        ]
    )

    if summary["missed_mutants"]:
        lines.append("### Missed mutants")
        lines.append("")
        for name in summary["missed_mutants"]:
            lines.append(f"- `{name}`")
        lines.append("")

    if summary["timeout_mutants"]:
        lines.append("### Timed-out mutants (investigate)")
        lines.append("")
        for name in summary["timeout_mutants"]:
            lines.append(f"- `{name}`")
        lines.append("")

    lines.append(
        "This job is currently **informational only**: it reports the "
        "mutation score as a build artifact and does not fail the build. "
        "See `contracts/tansu/MUTATION_TESTING.md` for the baseline and "
        "methodology."
    )

    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "outcomes_json",
        type=Path,
        help="Path to cargo-mutants' mutants.out/outcomes.json",
    )
    args = parser.parse_args()

    if not args.outcomes_json.exists():
        print(
            f"ℹ️ No mutation report found at {args.outcomes_json} "
            "(cargo-mutants may have failed to run)."
        )
        return 0

    outcomes = json.loads(args.outcomes_json.read_text())
    summary = summarize(outcomes)
    print(generate_markdown(summary))
    return 0


if __name__ == "__main__":
    sys.exit(main())
