#!/usr/bin/env python3
"""Compare two preserved release binaries using alternating, sequential runs."""
import argparse
import hashlib
import json
import math
import statistics
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("before", type=Path)
parser.add_argument("after", type=Path)
parser.add_argument("manifest", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("--rounds", type=int, default=7)
parser.add_argument("--iterations", type=int, default=100)
args = parser.parse_args()
assert args.rounds > 0 and args.iterations > 0
args.output.mkdir(parents=True, exist_ok=True)
cases = json.loads(args.manifest.read_text())
binaries = {"before": args.before.resolve(), "after": args.after.resolve()}


def run(label, name, mode, suffix):
    case = cases[name]
    output = args.output / f"{name}-{mode}-{label}-{suffix}.json"
    subprocess.run(
        [str(binaries[label]), mode, case["file"], case["url"], str(args.iterations), str(output)],
        check=True,
    )
    return json.loads(output.read_text())


for name in cases:
    case = cases[name]
    assert hashlib.sha256(Path(case["file"]).read_bytes()).hexdigest() == case["sha256"], (
        f"Fixture changed since manifest creation: {name}"
    )
    before = run("before", name, "output", "correctness")
    after = run("after", name, "output", "correctness")
    assert before == after, f"Extraction output changed: {name} (inspect saved JSON)"
print(f"Identical extraction JSON for {len(cases)} workloads × 4 option sets", flush=True)

samples = {}
for round_number in range(args.rounds):
    order = ["before", "after"] if round_number % 2 == 0 else ["after", "before"]
    for name in cases:
        for mode in ("extract", "jsonld"):
            for label in order:
                result = run(label, name, mode, round_number)
                samples.setdefault((name, mode, label), []).append(result["samples_ns"])
    print(f"Completed alternating round {round_number + 1}/{args.rounds}", flush=True)

rows = []
for name in cases:
    for mode in ("extract", "jsonld"):
        row = {"workload": name, "mode": mode, "bytes": cases[name]["bytes"],
               "synthetic": cases[name]["synthetic"]}
        medians = {}
        for label in binaries:
            rounds = samples[name, mode, label]
            medians[label] = [statistics.median(values) for values in rounds]
            ordered = sorted(value for values in rounds for value in values)
            row[label] = {
                "median_ms": statistics.median(medians[label]) / 1e6,
                "p95_ms": ordered[math.ceil(len(ordered) * 0.95) - 1] / 1e6,
                "round_median_ms": [value / 1e6 for value in medians[label]],
            }
        improvements = [100 * (1 - after / before)
                        for before, after in zip(medians["before"], medians["after"])]
        row["paired_improvement_percent"] = {
            "median": statistics.median(improvements), "min": min(improvements), "max": max(improvements)
        }
        rows.append(row)
(args.output / "summary.json").write_text(json.dumps(rows, indent=2) + "\n")
print("workload | mode | before ms | after ms | median paired improvement")
for row in rows:
    print(f"{row['workload']} | {row['mode']} | {row['before']['median_ms']:.4f} | "
          f"{row['after']['median_ms']:.4f} | {row['paired_improvement_percent']['median']:.1f}%")
