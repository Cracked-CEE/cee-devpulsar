import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path


def extract_summary_from_stream(stream):
    cpu_measurements = []
    mem_measurements = []
    operation_costs = defaultdict(lambda: {"cpu": 0, "mem": 0})

    current_operation = None

    for line in stream:
        # Look for operation labels
        if "Cost Estimate -" in line:
            current_operation = line.split("Cost Estimate -")[1].strip()

        # Extract CPU usage
        if "Cpu limit" in line and "used" in line:
            cpu_match = re.search(r"used: (\d+)", line)
            if cpu_match:
                cpu_used = int(cpu_match.group(1))
                cpu_measurements.append(cpu_used)
                if current_operation:
                    operation_costs[current_operation]["cpu"] = cpu_used

        # Extract memory usage
        elif "Mem limit" in line and "used" in line:
            mem_match = re.search(r"used: (\d+)", line)
            if mem_match:
                mem_used = int(mem_match.group(1))
                mem_measurements.append(mem_used)
                if current_operation:
                    operation_costs[current_operation]["mem"] = mem_used
                    current_operation = None  # Reset after collecting both CPU and mem

    return cpu_measurements, mem_measurements, operation_costs


def generate_markdown(cpu_measurements, mem_measurements, operation_costs):
    if not cpu_measurements and not mem_measurements:
        return "No cost data found in test output."

    lines = []

    # Overall summary
    if cpu_measurements and mem_measurements:
        total_cpu = sum(cpu_measurements)
        total_mem = sum(mem_measurements)
        max_cpu = max(cpu_measurements)
        max_mem = max(mem_measurements)
        avg_cpu = total_cpu / len(cpu_measurements)
        avg_mem = total_mem / len(mem_measurements)

        lines.extend(
            [
                "## 📊 Soroban Cost Estimation Summary",
                "",
                "| Metric | Total | Maximum | Average | Count |",
                "|--------|-------|---------|---------|-------|",
                f"| CPU Instructions | {total_cpu:,} | {max_cpu:,} | {avg_cpu:,.0f} | {len(cpu_measurements)} |",
                f"| Memory Bytes | {total_mem:,} | {max_mem:,} | {avg_mem:,.0f} | {len(mem_measurements)} |",
                "",
            ]
        )

    # Per-operation breakdown
    if operation_costs:
        lines.extend(
            [
                "## 🔧 Cost Breakdown by Operation",
                "",
                "| Operation | CPU Instructions | Memory Bytes |",
                "|-----------|------------------|--------------|",
            ]
        )

        # Sort operations by CPU usage (highest first)
        sorted_ops = sorted(
            operation_costs.items(), key=lambda x: x[1]["cpu"], reverse=True
        )

        for operation, costs in sorted_ops:
            lines.append(f"| {operation} | {costs['cpu']:,} | {costs['mem']:,} |")

        lines.append("")

    # Performance indicators
    if cpu_measurements:
        cpu_efficiency = (
            max(cpu_measurements) / 100_000_000
        ) * 100  # Percentage of CPU limit
        mem_efficiency = (
            max(mem_measurements) / 41_943_040
        ) * 100  # Percentage of memory limit

        lines.extend(
            [
                "## ⚡ Resource Utilization",
                "",
                f"- **CPU Utilization**: {cpu_efficiency:.2f}% of limit",
                f"- **Memory Utilization**: {mem_efficiency:.2f}% of limit",
                "",
            ]
        )

        # Add performance warnings
        if cpu_efficiency > 50:
            lines.append(
                "⚠️ **High CPU usage detected** - Consider optimizing computational complexity"
            )
        if mem_efficiency > 50:
            lines.append(
                "⚠️ **High memory usage detected** - Consider optimizing data structures"
            )
        if cpu_efficiency <= 10 and mem_efficiency <= 10:
            lines.append(
                "✅ **Excellent resource efficiency** - Low resource consumption"
            )

    return "\n".join(lines)


def enforce_cost_budget(
    baseline_costs: dict[str, dict],
    pr_costs: dict[str, dict],
    threshold_pct: float = 5.0,
) -> list[str]:
    """Compare pr_costs against baseline_costs per entrypoint and return a list of
    human-readable regressions that exceed threshold_pct. Entrypoints present in
    pr_costs but absent from baseline_costs (new entrypoints) are skipped here —
    callers should report those separately as informational, not as regressions.
    """
    regressions = []

    for entrypoint in sorted(pr_costs):
        baseline = baseline_costs.get(entrypoint)
        if baseline is None:
            continue

        pr = pr_costs[entrypoint]
        for metric in ("cpu", "mem"):
            base_val = baseline.get(metric, 0)
            pr_val = pr.get(metric, 0)
            if base_val <= 0:
                continue

            pct_change = ((pr_val - base_val) / base_val) * 100
            if pct_change > threshold_pct:
                regressions.append(
                    f"`{entrypoint}` {metric} usage regressed by {pct_change:.1f}% "
                    f"(baseline: {base_val:,}, pr: {pr_val:,}, "
                    f"allowed threshold: {threshold_pct:.1f}%)"
                )

    return regressions


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--baseline",
        type=Path,
        default=None,
        help="Path to a baseline costs JSON file to compare against. If omitted "
        "or the file doesn't exist, budget enforcement is skipped (bootstrap run).",
    )
    parser.add_argument(
        "--threshold",
        type=float,
        default=5.0,
        help="Allowed regression percentage before a cost is flagged (default: 5.0)",
    )
    parser.add_argument(
        "--write-costs",
        type=Path,
        default=None,
        help="Write the parsed per-entrypoint costs from this run to a JSON file "
        "(e.g. to publish as next run's baseline).",
    )
    args = parser.parse_args()

    cpu_measurements, mem_measurements, operation_costs = extract_summary_from_stream(
        sys.stdin
    )
    pr_costs = {op: dict(costs) for op, costs in operation_costs.items()}

    print(generate_markdown(cpu_measurements, mem_measurements, operation_costs))

    if args.write_costs:
        args.write_costs.write_text(json.dumps(pr_costs, indent=2, sort_keys=True))

    if not args.baseline:
        return 0

    if not args.baseline.exists():
        print("\n## 💰 Cost Budget Enforcement\n")
        print(
            "ℹ️ No baseline available yet — skipping regression enforcement "
            "(this looks like a bootstrap run)."
        )
        return 0

    baseline_costs = json.loads(args.baseline.read_text())
    new_entrypoints = sorted(set(pr_costs) - set(baseline_costs))
    regressions = enforce_cost_budget(baseline_costs, pr_costs, args.threshold)

    print("\n## 💰 Cost Budget Enforcement\n")

    if new_entrypoints:
        print(
            "ℹ️ New entrypoint(s) with no baseline yet (informational only, "
            f"not evaluated as regressions): {', '.join(new_entrypoints)}\n"
        )

    if regressions:
        print(
            f"❌ **{len(regressions)} cost regression(s) exceeded the "
            f"{args.threshold:.1f}% threshold:**\n"
        )
        for regression in regressions:
            print(f"- {regression}")
        return 1

    print(f"✅ No cost regressions beyond the {args.threshold:.1f}% threshold.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
