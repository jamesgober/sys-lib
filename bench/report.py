#!/usr/bin/env python3
"""Tables from criterion's output, and the 5% regression gate.

    report.py table <criterion-dir>            markdown table of every benchmark
    report.py gate <criterion-dir> <rounds>    fail when a sys-lib benchmark is
                                               more than 5% slower in every round

Each round runs the base build and the change, in alternating order, saving
their estimates as `base-N` and `pr-N`; the gate compares the medians round by
round, so a slowdown counts only when it repeats: one noisy round on a shared
runner does not fail a change (dev/PLAN.md section 18).
"""

import json
import os
import sys

LIMIT = 0.05


def benchmarks(root):
    """(group, function, directory) for each benchmark criterion wrote."""
    out = []
    for group in sorted(os.listdir(root)):
        gdir = os.path.join(root, group)
        if not os.path.isdir(gdir) or group == "report":
            continue
        for func in sorted(os.listdir(gdir)):
            fdir = os.path.join(gdir, func)
            if os.path.isdir(fdir) and func != "report":
                out.append((group, func, fdir))
    return out


def load(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def human(ns):
    for unit, scale in (("ms", 1e6), ("µs", 1e3)):
        if ns >= scale:
            return f"{ns / scale:.2f} {unit}"
    return f"{ns:.1f} ns"


def table(root):
    rows = []
    for group, func, fdir in benchmarks(root):
        est = load(os.path.join(fdir, "new", "estimates.json"))
        if est is None:
            continue
        rows.append((group, func.replace("_", " "), est["median"]["point_estimate"]))
    print("| reading | implementation | median |")
    print("|---|---|---|")
    for group, func, ns in rows:
        print(f"| {group} | {func} | {human(ns)} |")


def gate(root, rounds):
    failed = []
    print("| benchmark | change per round |")
    print("|---|---|")
    for group, func, fdir in benchmarks(root):
        if not (func.startswith("sys-lib") or group == "reports"):
            continue
        changes = []
        for n in range(1, rounds + 1):
            base = load(os.path.join(fdir, f"base-{n}", "estimates.json"))
            pr = load(os.path.join(fdir, f"pr-{n}", "estimates.json"))
            if base is not None and pr is not None:
                # Medians: one descheduled sample moves a mean, not a median.
                b = base["median"]["point_estimate"]
                changes.append(pr["median"]["point_estimate"] / b - 1 if b > 0 else 0.0)
        if not changes:
            continue
        print(f"| {group}/{func} | " + ", ".join(f"{c * 100:+.1f}%" for c in changes) + " |")
        if len(changes) == rounds and all(c > LIMIT for c in changes):
            failed.append(f"{group}/{func}")
    if failed:
        print(f"\nSlower by more than {LIMIT:.0%} in every round: " + ", ".join(failed))
        return 1
    print(f"\nNo sys-lib benchmark slower by more than {LIMIT:.0%} in every round.")
    return 0


if __name__ == "__main__":
    if len(sys.argv) >= 3 and sys.argv[1] == "table":
        table(sys.argv[2])
    elif len(sys.argv) >= 4 and sys.argv[1] == "gate":
        sys.exit(gate(sys.argv[2], int(sys.argv[3])))
    else:
        print(__doc__)
        sys.exit(2)
