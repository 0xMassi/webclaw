import json, os, re, statistics, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CASES = json.loads((ROOT / "cloudflare-cases.json").read_text())
PYTHON = {"webclaw-http": "venv-base", "webclaw-render": "venv-base", "webclaw-solve": "venv-base",
          "scrapling": "venv-scrapling", "crawl4ai": "venv-crawl4ai"}
ARMS = os.environ.get("CF_ARMS", "webclaw-http,webclaw-render,webclaw-solve,scrapling,crawl4ai").split(",")
TOKEN = re.compile(r'name="(?:cf-turnstile|g-recaptcha)-response"[^>]*value="[^"]{20,}"'
                   r'|value="[^"]{20,}"[^>]*name="(?:cf-turnstile|g-recaptcha)-response"')
KINDS = ["non-interactive", "managed", "interactive", "embedded"]


def solved(case, text, token=None):
    if case["marker"] == "token":
        return len(token or "") >= 20 or bool(TOKEN.search(text))
    return case["marker"] in text and "<title>Just a moment...</title>" not in text


def reuse_in_webclaw(case):
    """Does webclaw, after solving the page once, fetch it again over plain HTTP with the cookies it got?"""
    again = case["url"] + ("&" if "?" in case["url"] else "?") + "again=1"
    cmd = [os.environ["WEBCLAW_BIN"], case["url"], again, "--solve", "--concurrency", "1", "-f", "json",
           "--timeout", "60"]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=300,
                       env={**os.environ, "WEBCLAW_LOG": "webclaw_fetch=debug"})
    lines = re.sub(r"\x1b\[[0-9;]*m", "", r.stderr).splitlines()
    sent = any(again in l and "sending the cookies" in l for l in lines)
    log = [l for l in lines if again in l and "fetch complete" in l]
    return {"same_ua": sent and any("status=200" in l for l in log), "own_ua": None}


def reuse(case, row):
    """Can webclaw's plain HTTP client reach the page with the browser's cookies?"""
    if row.get("arm") == "webclaw-solve":
        return reuse_in_webclaw(case) if case["kind"] != "embedded" else None
    if "cf_clearance" not in row["cookies"] or not row["ua"]:
        return None
    cookie = "; ".join(f"{k}={v}" for k, v in row["cookies"].items())
    cmd = [os.environ["WEBCLAW_BIN"], case["url"], "--render", "never", "-f", "json", "--cookie", cookie]
    if "Macintosh" in row["ua"]:
        cmd += ["--browser", "chrome-macos"]
    same_ua = subprocess.run(cmd + ["-H", f"User-Agent: {row['ua']}"], capture_output=True, text=True, timeout=60)
    own_ua = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
    return {"same_ua": solved(case, same_ua.stdout), "own_ua": solved(case, own_ua.stdout)}


def run(arm):
    out = WORK / "cloudflare-out" / arm
    python = WORK / PYTHON[arm] / "bin/python"
    r = subprocess.run([str(python), "-I", str(ROOT / "scripts/cloudflare_arms.py"), arm, str(out),
                        str(ROOT / "cloudflare-cases.json")], capture_output=True, text=True, timeout=3 * 3600)
    rows = {}
    for line in r.stdout.splitlines():
        if line.startswith("{"):
            row = json.loads(line)
            rows[row["url"]] = row
    results = []
    for case in CASES:
        row = rows.get(case["url"], {"file": None, "ms": 0, "error": f"no result, exit {r.returncode}",
                                     "cookies": {}, "ua": None, "token": None})
        row["arm"] = arm
        text = Path(row["file"]).read_text(errors="replace") if row["file"] else ""
        ok = solved(case, text, row.get("token"))
        results.append({"arm": arm, "url": case["url"], "kind": case["kind"], "solved": ok, "ms": round(row["ms"]),
                        "error": None if ok else (row["error"] or "challenge still shown")[-160:],
                        "reuse": reuse(case, row) if ok else None})
    return results


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows = []
    for arm in ARMS:
        if not (WORK / PYTHON[arm]).exists():
            continue
        got = run(arm)
        rows += got
        for r in got:
            print(json.dumps(r), flush=True)
    (res / "cloudflare-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    arms = [a for a in ARMS if any(r["arm"] == a for r in rows)]
    counts = {k: sum(c["kind"] == k for c in CASES) for k in KINDS}
    lines = [f"# Cloudflare challenges {label}", "",
             f"Each tool loads {len(CASES)} pages behind a Cloudflare challenge or with a Turnstile widget, one "
             "fresh browser per page, 60 s budget per page. A challenge page counts as solved when the tool's output "
             "holds a text marker from the real page and not the challenge's `Just a moment...` title. An embedded "
             "Turnstile widget counts as solved when the page holds a widget token. ms is the median over solved "
             "pages. `reuse` counts solved challenge pages whose browser cookies, sent with the browser's User-Agent, "
             "let webclaw's plain HTTP client (no browser) load the page straight after: for webclaw `--solve`, "
             "the second of two URLs of the page in one run (`--concurrency 1`); for Scrapling, a separate webclaw "
             "run handed its cookies and User-Agent, with the `chrome-macos` profile when that User-Agent is "
             "macOS. `reuse, own UA` sends Scrapling's cookies with webclaw's own User-Agent instead. webclaw "
             "`--solve` loads embedded widgets with `--render always` and reads the token with an `eval` action.",
             "",
             "| tool | " + " | ".join(f"{k} ({counts[k]})" for k in KINDS) + " | all | ms (median) | reuse | reuse, own UA |",
             "|---|" + "---:|" * (len(KINDS) + 4)]
    for arm in arms:
        mine = [r for r in rows if r["arm"] == arm]
        ok = [r for r in mine if r["solved"]]
        by_kind = [f"{sum(r['solved'] for r in mine if r['kind'] == k)}/{counts[k]}" for k in KINDS]
        reused = [r["reuse"] for r in ok if r["reuse"] is not None]
        ms = statistics.median([r["ms"] for r in ok]) if ok else 0
        own = [x["own_ua"] for x in reused if x["own_ua"] is not None]
        reuse_cells = [f"{sum(x['same_ua'] for x in reused)}/{len(reused)}" if reused else "-",
                       f"{sum(own)}/{len(own)}" if own else "-"]
        lines.append(f"| {arm} | " + " | ".join(by_kind) + f" | {len(ok)}/{len(mine)} | {ms:.0f} | "
                     + " | ".join(reuse_cells) + " |")
    lines += ["", "Per page (solved, ms):", "", "| page | kind | " + " | ".join(arms) + " |",
              "|---|---|" + "---|" * len(arms)]
    for case in CASES:
        cells = []
        for arm in arms:
            r = next(r for r in rows if r["arm"] == arm and r["url"] == case["url"])
            cells.append(f"yes {r['ms'] / 1000:.1f}s" if r["solved"] else "no")
        lines.append(f"| {case['url']} | {case['kind']} | " + " | ".join(cells) + " |")
    (res / "cloudflare-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
