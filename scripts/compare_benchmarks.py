#!/usr/bin/env python3
"""Compare two AuraDB benchmark baselines and report regressions.

A dependency-free alternative to `auradb bench compare` for CI: it reads the
committed JSON baselines (see `benches/baseline/`) and compares per-benchmark
values, accounting for each measurement's unit:

  * ``ops_per_sec`` — higher is better; a drop is a regression.
  * ``ns_per_op`` / ``seconds`` — lower is better; a rise is a regression.

By default it only warns. Pass ``--fail-threshold-percent N`` to exit non-zero
when any benchmark regresses by more than N percent.

Benchmarks are hardware-sensitive: only compare baselines produced on the same
machine and build profile (see ``benches/baseline/README.md``).

Usage:
    scripts/compare_benchmarks.py BASELINE.json CURRENT.json
    scripts/compare_benchmarks.py BASELINE.json CURRENT.json --fail-threshold-percent 10
"""

from __future__ import annotations

import argparse
import json
import sys

# Units where a smaller value is the improvement.
LOWER_IS_BETTER = {"ns_per_op", "seconds"}


def load(path: str) -> dict:
    with open(path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def measurements(report: dict) -> dict:
    out = {}
    for m in report.get("measurements", []):
        out[m["name"]] = m
    return out


def regression_percent(unit: str, base: float, cur: float) -> float:
    """Percent regression (positive = worse). 0 if base is non-positive."""
    if base <= 0:
        return 0.0
    if unit in LOWER_IS_BETTER:
        return (cur - base) / base * 100.0
    return (base - cur) / base * 100.0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("baseline", help="reference baseline JSON")
    parser.add_argument("current", help="current report JSON")
    parser.add_argument("--fail-threshold-percent", type=float, default=None,
                        help="exit non-zero if any benchmark regresses beyond this percent")
    args = parser.parse_args()

    base = load(args.baseline)
    cur = load(args.current)
    base_m = measurements(base)
    cur_m = measurements(cur)

    print("benchmark comparison: {} ({}) -> {} ({})".format(
        base.get("auradb_version", "?"), base.get("commit", "?"),
        cur.get("auradb_version", "?"), cur.get("commit", "?")))
    print("hardware-sensitive: compare only reports from the same machine/profile.")

    worst = 0.0
    worst_name = None
    missing = sorted(set(base_m) - set(cur_m))
    for name in sorted(base_m):
        if name not in cur_m:
            continue
        unit = base_m[name].get("unit", "")
        b = float(base_m[name]["value"])
        c = float(cur_m[name]["value"])
        reg = regression_percent(unit, b, c)
        sign = "+" if reg <= 0 else "-"  # report change in the "better" direction
        delta = abs(reg)
        marker = "  REGRESSION" if reg > 0 else ""
        print("  {}: {:.2f} -> {:.2f} {} ({}{:.1f}% vs baseline){}".format(
            name, b, c, unit, "-" if reg > 0 else "+", delta, marker))
        if reg > worst:
            worst, worst_name = reg, name

    for name in missing:
        print("  {}: present in baseline, missing in current (skipped)".format(name))

    if args.fail_threshold_percent is None:
        print("worst regression: {:.1f}% ({}); warnings only (no fail threshold set)".format(
            worst, worst_name or "none"))
        return 0

    if worst > args.fail_threshold_percent:
        print("FAIL: worst regression {:.1f}% ({}) exceeds threshold {:.1f}%".format(
            worst, worst_name, args.fail_threshold_percent))
        return 1
    print("OK: worst regression {:.1f}% ({}) within threshold {:.1f}%".format(
        worst, worst_name or "none", args.fail_threshold_percent))
    return 0


if __name__ == "__main__":
    sys.exit(main())
