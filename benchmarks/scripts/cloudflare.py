import json, os, re, statistics, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CASES = json.loads((ROOT / "cloudflare-cases.json").read_text())
PYTHON = {"webclaw-http": "venv-base", "webclaw-render": "venv-base", "webclaw-solve": "venv-base",
          "scrapling": "venv-scrapling", "scrapling-real-chrome": "venv-scrapling", "crawl4ai": "venv-crawl4ai"}
ARMS = os.environ.get(
    "CF_ARMS", "webclaw-http,webclaw-render,webclaw-solve,scrapling,scrapling-real-chrome,crawl4ai").split(",")
TOKEN = re.compile(r'name="(?:cf-turnstile|g-recaptcha)-response"[^>]*value="[^"]{20,}"'
                   r'|value="[^"]{20,}"[^>]*name="(?:cf-turnstile|g-recaptcha)-response"')
KINDS = [k for k in ["non-interactive", "managed", "interactive", "embedded"] if any(c["kind"] == k for c in CASES)]


def solved(case, text, token=None):
    if case["marker"] == "token":
        return len(token or "") >= 20 or bool(TOKEN.search(text))
    markers = case["marker"] if isinstance(case["marker"], list) else [case["marker"]]
    return any(m in text for m in markers) and "<title>Just a moment...</title>" not in text


def again(url):
    return url + ("&" if "?" in url else "?") + "again=1"


def webclaw_reuse(case):
    """webclaw solves the page, then fetches it again over plain HTTP in the same run with the clearance."""
    cmd = [os.environ["WEBCLAW_BIN"], case["url"], again(case["url"]), "--solve", "--concurrency", "1",
           "-f", "json", "--timeout", "60"]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=300,
                       env={**os.environ, "WEBCLAW_LOG": "webclaw_fetch=debug"})
    lines = re.sub(r"\x1b\[[0-9;]*m", "", r.stderr).splitlines()
    sent = any(again(case["url"]) in l and "sending the clearance" in l for l in lines)
    rendered_again = any(again(case["url"]) in l and "fetch complete status=403" in l for l in lines)
    try:
        pages = json.loads(r.stdout)
    except json.JSONDecodeError:
        pages = []
    second = next((json.dumps(p.get("result")) for p in pages if p.get("url") == again(case["url"])), "")
    return sent and not rendered_again and solved(case, second)


def run(arm, label):
    out = WORK / "cloudflare-out" / label / arm
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
                                     "token": None, "reuse_file": None})
        text = Path(row["file"]).read_text(errors="replace") if row["file"] else ""
        ok = solved(case, text, row.get("token"))
        reuse = None
        if ok and case["kind"] != "embedded":
            if arm == "webclaw-solve":
                reuse = webclaw_reuse(case)
            elif row.get("reuse_file"):
                reuse = solved(case, Path(row["reuse_file"]).read_text(errors="replace"))
        results.append({"arm": arm, "url": case["url"], "kind": case["kind"], "solved": ok, "ms": round(row["ms"]),
                        "error": None if ok else (row["error"] or "challenge still shown")[-160:], "reuse": reuse})
    return results


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows = []
    for arm in ARMS:
        if not (WORK / PYTHON[arm]).exists():
            continue
        got = run(arm, label)
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
             "pages. `reuse` counts solved challenge pages that the tool's own plain HTTP client (no browser) then "
             "loads in the same process, at the page's URL with `?again=1`, with the browser's cookies and "
             "User-Agent, under the same marker check: webclaw as the second URL of one `--solve` run "
             "(`--concurrency 1`, counted only when it sent the clearance and got no new challenge), Scrapling with "
             "its `Fetcher` (`impersonate=\"chrome\"`). webclaw `--solve` loads embedded widgets with "
             "`--render always` and reads the token with an `executeJavascript` step.",
             "",
             "| tool | " + " | ".join(f"{k} ({counts[k]})" for k in KINDS) + " | all | ms (median) | reuse |",
             "|---|" + "---:|" * (len(KINDS) + 3)]
    for arm in arms:
        mine = [r for r in rows if r["arm"] == arm]
        ok = [r for r in mine if r["solved"]]
        by_kind = [f"{sum(r['solved'] for r in mine if r['kind'] == k)}/{counts[k]}" for k in KINDS]
        reused = [r["reuse"] for r in ok if r["reuse"] is not None]
        ms = statistics.median([r["ms"] for r in ok]) if ok else 0
        reuse_cell = f"{sum(reused)}/{len(reused)}" if reused else "-"
        lines.append(f"| {arm} | " + " | ".join(by_kind) + f" | {len(ok)}/{len(mine)} | {ms:.0f} | {reuse_cell} |")
    lines += ["", "Per page (solved, ms, reuse):", "", "| page | kind | " + " | ".join(arms) + " |",
              "|---|---|" + "---|" * len(arms)]
    for case in CASES:
        cells = []
        for arm in arms:
            r = next(r for r in rows if r["arm"] == arm and r["url"] == case["url"])
            reuse = {True: ", reused", False: ", not reused", None: ""}[r["reuse"]]
            cells.append(f"yes {r['ms'] / 1000:.1f}s{reuse}" if r["solved"] else "no")
        lines.append(f"| {case['url']} | {case['kind']} | " + " | ".join(cells) + " |")
    (res / "cloudflare-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
