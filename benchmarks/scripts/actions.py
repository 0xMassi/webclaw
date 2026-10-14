import json, os, statistics, subprocess, sys
from pathlib import Path

from bs4 import BeautifulSoup

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CASES_FILE = ROOT / "actions-cases.json"
CASES = json.loads(CASES_FILE.read_text())
ARMS = {"webclaw": "venv-base", "webclaw-none": "venv-base", "scrapling": "venv-scrapling",
        "crawl4ai": "venv-crawl4ai"}
ROUNDS = int(os.environ.get("ACTIONS_ROUNDS", "2"))
ONLY = os.environ.get("ACTIONS_ONLY", "").split()


def score(case, file):
    if not file:
        return 0, 0.0
    soup = BeautifulSoup(Path(file).read_text(), "html.parser")
    found = soup.select(case["items"])
    if isinstance(case["truth"], list):
        keys = {(el.get(case["attr"]) if case.get("attr") else el.get_text(" ", strip=True)) for el in found}
        return len(keys), len(keys & set(case["truth"])) / max(len(keys), len(case["truth"]))
    return len(found), min(len(found), case["truth"]) / case["truth"]


def run(arm, out):
    python = WORK / ARMS[arm] / "bin/python"
    cmd = [str(python), str(ROOT / "scripts/actions_arms.py"), arm, str(out), str(CASES_FILE)] + ONLY
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=3600)
    done = {}
    for line in r.stdout.splitlines():
        if line.startswith("{"):
            row = json.loads(line)
            done[row["case"]] = row
    if len(done) < len([c for c in CASES if not ONLY or c["name"] in ONLY]):
        print(f"{arm}: exit {r.returncode}\n{r.stderr[-2000:]}", file=sys.stderr)
    return done


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows = []
    for rnd in range(ROUNDS):
        for arm in ARMS:
            done = run(arm, WORK / "actions-out" / arm)
            for case in CASES:
                if ONLY and case["name"] not in ONLY:
                    continue
                got = done.get(case["name"], {"file": None, "ms": None, "error": "no result"})
                items, frac = score(case, got["file"])
                truth = len(case["truth"]) if isinstance(case["truth"], list) else case["truth"]
                row = {"round": rnd, "arm": arm, "case": case["name"], "items": items, "truth": truth,
                       "score": round(frac, 3), "ok": frac == 1.0, "ms": got["ms"], "error": got["error"]}
                rows.append(row)
                print(json.dumps(row), flush=True)
    (res / "actions-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    arms = list(ARMS)
    lines = [f"# Page actions {label}", "",
             f"Each tool loads {len({r['case'] for r in rows})} pages that need interaction in a fresh headless "
             f"browser, runs the same steps, and returns the page's HTML; {ROUNDS} rounds. `items` counts the "
             "elements the case's selector finds in that HTML (the median over rounds), `ok` the rounds with every "
             "expected item, `s` the median wall time. `webclaw-none` renders with no actions. Ground truth and "
             "how it was checked are in `actions-cases.json`.", "",
             "| case | kind | truth | " + " | ".join(f"{a} items | {a} ok | {a} s" for a in arms) + " |",
             "|---|---|---:|" + "---:|---:|---:|" * len(arms)]
    totals = {a: [0, 0, []] for a in arms}
    for case in CASES:
        cells = []
        for a in arms:
            rs = [r for r in rows if r["arm"] == a and r["case"] == case["name"]]
            if not rs:
                cells.append("- | - | -")
                continue
            ok = sum(r["ok"] for r in rs)
            ms = [r["ms"] for r in rs if r["ms"] is not None]
            totals[a][0] += ok
            totals[a][1] += len(rs)
            totals[a][2] += ms
            cells.append(f"{statistics.median(r['items'] for r in rs):g} | {ok}/{len(rs)} | "
                         f"{statistics.median(ms) / 1000 if ms else 0:.1f}")
        if any(c != "- | - | -" for c in cells):
            truth = len(case["truth"]) if isinstance(case["truth"], list) else case["truth"]
            lines.append(f"| {case['name']} | {case['kind']} | {truth} | " + " | ".join(cells) + " |")
    lines.append("| **all** | | | " + " | ".join(
        f" | **{t[0]}/{t[1]}** | {statistics.median(t[2]) / 1000 if t[2] else 0:.1f}" for t in totals.values()) + " |")
    errors = sorted({f"{r['arm']} {r['case']}: {r['error']}" for r in rows if r["error"]})
    if errors:
        lines += ["", "Errors:", ""] + [f"- {e}" for e in errors]
    (res / "actions-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
