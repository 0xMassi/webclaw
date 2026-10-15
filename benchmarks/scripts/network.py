import json, re, statistics, subprocess, sys, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CASES_FILE = ROOT / "network-cases.json"
CASES = json.loads(CASES_FILE.read_text())
ARMS = [("webclaw", "venv-base", False), ("scrapling", "venv-scrapling", False), ("crawl4ai", "venv-crawl4ai", False),
        ("webclaw", "venv-base", True), ("scrapling", "venv-scrapling", True)]
UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36"


def has_field(body, field):
    try:
        value = json.loads(body or "")
    except ValueError:
        return False
    if isinstance(value, list):
        value = value[0] if value else None
    return isinstance(value, dict) and field in value


def truth():
    rows = []
    for case in CASES:
        for api in case["apis"]:
            data = json.dumps(api["post"]).encode() if "post" in api else None
            headers = {"User-Agent": UA, "Accept": "application/json"}
            if data:
                headers["Content-Type"] = "application/json"
            try:
                with urllib.request.urlopen(urllib.request.Request(api["check"], data=data, headers=headers),
                                            timeout=30) as r:
                    ok = has_field(r.read().decode("utf-8", "replace"), api["field"])
                    error = None if ok else "field missing"
            except Exception as e:
                ok, error = False, str(e)[:120]
            rows.append({"page": case["page"], "api": api["check"], "field": api["field"], "ok": ok, "error": error})
    return rows


def score(case, run):
    found = ok = 0
    for api in case["apis"]:
        hits = [r for r in run["responses"] if re.search(api["match"], r["url"])]
        found += bool(hits)
        ok += any(has_field(r["body"], api["field"]) for r in hits)
    noise = sum(1 for r in run["responses"] if not any(re.search(a["match"], r["url"]) for a in case["apis"]))
    return {"apis": len(case["apis"]), "found": found, "ok": ok, "noise": noise, "captured": len(run["responses"])}


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    checked = truth()
    for row in checked:
        print(json.dumps(row), flush=True)
    rows, pages = [], {}
    for arm, venv, filtered in ARMS:
        cmd = [str(WORK / venv / "bin/python"), str(ROOT / "scripts/network_arms.py"), arm, "1" if filtered else "0",
               str(CASES_FILE)]
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=3600)
        runs = {d["page"]: d for d in map(json.loads, (l for l in r.stdout.splitlines() if l.startswith("{")))}
        if len(runs) < len(CASES):
            print(f"{arm}: exit {r.returncode}, {len(CASES) - len(runs)} pages missing\n{r.stderr[-2000:]}", file=sys.stderr)
        name = f"{arm} (filtered)" if filtered else arm
        per = []
        for case in CASES:
            run = runs.get(case["page"]) or {"ms": 0, "bytes": 0, "error": "no result", "responses": []}
            s = {"page": case["page"], **score(case, run), "ms": run["ms"], "kb": run["bytes"] / 1024, "error": run["error"]}
            per.append(s)
            pages.setdefault(case["page"], {})[name] = s
        row = {"arm": name, "pages": len(per), "failed": sum(1 for s in per if s["error"]),
               "apis": sum(s["apis"] for s in per), "found": sum(s["found"] for s in per),
               "ok": sum(s["ok"] for s in per), "captured": sum(s["captured"] for s in per),
               "noise": sum(s["noise"] for s in per), "ms": statistics.median(s["ms"] for s in per),
               "kb": statistics.median(s["kb"] for s in per), "errors": [s["error"][:160] for s in per if s["error"]][:4]}
        rows.append(row)
        print(json.dumps(row), flush=True)
    (res / "network-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    (res / "network-pages.json").write_text(json.dumps(pages, indent=1))
    verified = sum(c["ok"] for c in checked)
    lines = [f"# Network capture {label}", "",
             f"Each tool renders {len(CASES)} pages whose data comes from JSON APIs, with a fresh browser per page, and "
             "returns the XHR and fetch responses the page received. The ground truth is one API per page and a field "
             f"in its JSON, checked by calling each API directly: {verified}/{len(checked)} answered with the field "
             "this run. `found` counts ground-truth APIs whose URL is in the output, `bodies ok` those whose captured "
             "body parses as JSON with the field, `noise` the other responses in the output. webclaw runs "
             "`--capture-network` (xhr and fetch). Scrapling runs `DynamicFetcher.fetch(capture_xhr=...)` with "
             "`network_idle=True`. Crawl4AI runs `capture_network_requests=True` with `wait_until=\"networkidle\"`; "
             "it keeps every response of every resource type and has no filter. The filtered rows pass each page's "
             "ground-truth pattern as the tool's URL filter.", "",
             "| tool | pages ok | found | bodies ok | responses | noise | ms (median) | output KB (median) |",
             "|---|---:|---:|---:|---:|---:|---:|---:|"]
    for r in rows:
        lines.append(f"| {r['arm']} | {r['pages'] - r['failed']}/{r['pages']} | {r['found']}/{r['apis']} | "
                     f"{r['ok']}/{r['apis']} | {r['captured']} | {r['noise']} | {r['ms']:.0f} | {r['kb']:.0f} |")
    names = [f"{a} (filtered)" if f else a for a, _, f in ARMS]
    lines += ["", "Bodies ok per page (responses captured):", "", "| page | " + " | ".join(names) + " |",
              "|---|" + "---|" * len(names)]
    for case in CASES:
        cells = []
        for n in names:
            s = pages[case["page"]][n]
            cells.append("fail" if s["error"] else f"{s['ok']}/{s['apis']} ({s['captured']})")
        lines.append(f"| {case['page']} | " + " | ".join(cells) + " |")
    lines += ["", "Direct API checks:", ""] + [f"- {c['api']}: {'ok' if c['ok'] else c['error']}" for c in checked]
    (res / "network-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
