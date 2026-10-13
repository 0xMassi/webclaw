"""Fetch the url-mode cases once per browser profile, with webclaw and with Scrapling's matching impersonation.

usage: profiles.py <label>
A run counts as ok when the tool returns text and the page keeps at least one of its case facts.
"""
import json, os, statistics, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TIMEOUT = 30
PROFILES = {"chrome": "chrome", "chrome-macos": None, "firefox": "firefox", "safari": "safari",
            "safari-ios": "safari_ios", "edge": "edge", "random": None}
CASES = [json.loads(l) for l in (ROOT / "cases.jsonl").read_text().splitlines() if l.strip()]
CASES = [c for c in CASES if c["category"] != "pdf"]


def webclaw(profile, case):
    r = subprocess.run([os.environ["WEBCLAW_BIN"], case["url"], "-b", profile, "-f", "llm", "--render", "never",
                        "-t", str(TIMEOUT)], capture_output=True, text=True, timeout=TIMEOUT + 15)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip()[-200:])
    return r.stdout


def scrapling(impersonate, case):
    from scrapling.fetchers import Fetcher
    page = Fetcher.get(case["url"], impersonate=impersonate, timeout=TIMEOUT)
    if page.status >= 400:
        raise RuntimeError(f"HTTP {page.status}")
    return page.markdown()


def one(fetch, case):
    t0 = time.perf_counter()
    try:
        text, error = fetch(case), None
    except Exception as e:
        text, error = "", str(e)[-200:]
    seconds = time.perf_counter() - t0
    found = sum(f.lower() in text.lower() for f in case["facts"])
    return {"id": case["id"], "ok": bool(text) and found > 0, "facts": found, "of": len(case["facts"]),
            "seconds": seconds, "error": error}


def run(name, fetch):
    with ThreadPoolExecutor(8) as pool:
        rows = list(pool.map(lambda c: one(fetch, c), CASES))
    ok = [r for r in rows if r["ok"]]
    secs = sorted(r["seconds"] for r in ok) or [0]
    summary = {"arm": name, "runs": len(rows), "ok": len(ok),
               "recall": 100 * sum(r["facts"] for r in rows) / sum(r["of"] for r in rows),
               "p50": statistics.median(secs), "p95": secs[int(0.95 * (len(secs) - 1))],
               "failed": sorted(r["id"] for r in rows if not r["ok"])}
    print(json.dumps(summary), flush=True)
    return summary


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows = []
    for profile, impersonate in PROFILES.items():
        rows.append({"profile": profile, "tool": "webclaw",
                     **run(f"webclaw {profile}", lambda c, p=profile: webclaw(p, c))})
        if impersonate:
            rows.append({"profile": profile, "tool": "scrapling",
                         **run(f"scrapling {impersonate}", lambda c, i=impersonate: scrapling(i, c))})
    (res / "profiles-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# Browser profiles {label}", "",
             f"Each tool fetches the {len(CASES)} non-PDF url-mode cases once per browser identity, over HTTP only "
             "(webclaw `--render never -f llm`, Scrapling `Fetcher.get(impersonate=...)` then `.markdown()`). "
             "A run is ok when it returns text holding at least one of the case's facts; recall counts all facts. "
             "Scrapling has no separate macOS Chrome or random identity.", "",
             "| profile | tool | ok | fact recall % | p50 s | p95 s |", "|---|---|---:|---:|---:|---:|"]
    for r in rows:
        lines.append(f"| {r['profile']} | {r['tool']} | {r['ok']}/{r['runs']} | {r['recall']:.1f} | "
                     f"{r['p50']:.2f} | {r['p95']:.2f} |")
    (res / "profiles-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
