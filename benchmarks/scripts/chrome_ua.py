"""Does the local browser present itself as regular Chrome? Driven by ../chrome-ua.sh.

1. Headless checks: each tool renders four pages, a fresh browser per page, and runs the same probe
   script (scripts/chrome_ua_probe.js) in them. The probe reads the User-Agent, navigator.webdriver,
   the client hints and a dedicated worker's view; httpbin echoes the headers the browser sent; a public
   bot check lists the signals it caught; and two pages that answered headless Chrome differently
   (Nasdaq's quote page failed to load, Bundlephobia served a pre-rendered page that never calls its
   size API) show whether the page calls its data API.
2. Launch cost: one render of example.com per process, the way one CLI command renders.
3. The render slice of the extraction benchmark (run.py with the webclaw-auto and webclaw-render arms).

With WEBCLAW_BASELINE_BIN set, the older build runs too: as it is, and with --solve, which made it
launch Chrome twice to present itself as regular Chrome.
"""
import json, os, platform, re, statistics, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
PAGES = [
    {"name": "headers", "url": "https://httpbin.org/headers", "wait": 0},
    {"name": "bot-check", "url": "https://deviceandbrowserinfo.com/are_you_a_bot", "wait": 5000},
    {"name": "nasdaq", "url": "https://www.nasdaq.com/market-activity/stocks/aapl", "wait": 2000},
    {"name": "bundlephobia", "url": "https://bundlephobia.com/package/react", "wait": 2000},
]
ARMS = [("webclaw", "base", "webclaw"), ("webclaw-mcp", "mcp", "webclaw MCP `scrape`"),
        ("webclaw-before", "base", "webclaw before"), ("webclaw-before-solve", "base", "webclaw before, `--solve`"),
        ("scrapling-dynamic", "scrapling", "Scrapling DynamicFetcher"),
        ("scrapling-stealthy", "scrapling", "Scrapling StealthyFetcher"),
        ("crawl4ai", "crawl4ai", "Crawl4AI"), ("crawl4ai-stealth", "crawl4ai", "Crawl4AI `enable_stealth`")]
ROUNDS = int(os.environ.get("CHROME_UA_ROUNDS", "3"))
LAUNCHES = int(os.environ.get("CHROME_UA_LAUNCHES", "5"))
EXTRACTION_RUNS = os.environ.get("CHROME_UA_EXTRACTION_RUNS", "3")
BASELINE = os.environ.get("WEBCLAW_BASELINE_BIN")
MACHINE = {"Darwin": "macOS", "Linux": "Linux", "Windows": "Windows"}.get(platform.system())
ARCH = "arm" if platform.machine().lower() in ("arm64", "aarch64") else "x86"


def majors(brands):
    return {b.rsplit("/", 1)[1].split(".")[0] for b in brands or [] if "Brand" not in b}


def hint_majors(header):
    return {v for name, v in re.findall(r'"([^"]+)";v="(\d+)"', header or "") if "Brand" not in name}


def echoed(body):
    try:
        return json.JSONDecoder().raw_decode(body[body.index("{"):])[0]["headers"]
    except (ValueError, KeyError):
        return {}


def os_of(text):
    text = (text or "").lower()
    return next((name for key, name in (("mac", "macOS"), ("windows", "Windows"), ("linux", "Linux"), ("x11", "Linux"))
                 if key in text), None)


def tells(probe):
    headers = echoed(probe["body"])
    sent_ua, sent_hints = headers.get("User-Agent", ""), headers.get("Sec-Ch-Ua")
    worker = probe.get("worker") or {}
    ua_major = (re.search(r"Chrome/(\d+)", probe["ua"]) or [None, None])[1]
    found = []
    if "Headless" in probe["ua"] or "Headless" in sent_ua:
        found.append("UA says Headless")
    if "Headless" in (worker.get("ua") or ""):
        found.append("worker UA says Headless")
    if any("Headless" in b for b in (probe["brands"] or []) + (worker.get("brands") or [])) or "Headless" in (sent_hints or ""):
        found.append("client hints say Headless")
    if sent_ua != probe["ua"]:
        found.append("UA header differs from navigator.userAgent")
    if not sent_hints or not probe["brands"]:
        found.append("no client hints")
    elif {ua_major} != majors(probe["brands"]) or {ua_major} != hint_majors(sent_hints):
        found.append("UA version differs from client hints")
    if probe["platform"] != MACHINE or os_of(probe["ua"]) != MACHINE:
        found.append("platform differs from the machine's")
    if probe["architecture"] and probe["architecture"] != ARCH:
        found.append("architecture differs from the machine's")
    if worker.get("ua") != probe["ua"] or worker.get("brands") != probe["brands"]:
        found.append("worker differs from page")
    if probe["webdriver"]:
        found.append("navigator.webdriver is true")
    if not probe["full_version_list"] or not probe["platform_version"]:
        found.append("high-entropy client hints blank")
    return found


def bot_check(probe):
    body = probe["body"]
    try:
        verdict = json.JSONDecoder().raw_decode(body[body.index('{\n  "isBot"'):])[0]
    except ValueError:
        return None
    return verdict["isBot"], sorted(k for k, v in verdict.get("details", {}).items() if v is True)


DATA_API = {"nasdaq": "api.nasdaq.com/api/quote/AAPL/info", "bundlephobia": "bundlephobia.com/api/size?package=react&"}


def works(page, probe):
    return bool(probe) and any(DATA_API[page] in url for url in probe.get("api_calls") or [])


def slim(row):
    probe = row["probe"]
    if not probe:
        return row
    kept = {k: v for k, v in probe.items() if k not in ("body", "api_calls")}
    kept["sent"] = {k: v for k, v in echoed(probe["body"]).items() if k == "User-Agent" or k.startswith("Sec-Ch-Ua")}
    kept["data_api_called"] = works(row["page"], probe) if row["page"] in DATA_API else None
    kept["bot_check"] = bot_check(probe) if row["page"] == "bot-check" else None
    return {**row, "probe": kept}


def run_arm(arm, venv, pages_file):
    cmd = [str(WORK / f"venv-{venv}" / "bin/python"), str(ROOT / "scripts/chrome_ua_arms.py"), arm, str(ROUNDS), str(pages_file)]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=7200)
    rows = [json.loads(l) for l in r.stdout.splitlines() if l.startswith("{")]
    if len(rows) < ROUNDS * len(PAGES):
        print(f"{arm}: exit {r.returncode}, {len(rows)} of {ROUNDS * len(PAGES)} rows\n{r.stderr[-2000:]}", file=sys.stderr)
    return rows


def launch_cost(bin, extra):
    times = []
    for _ in range(LAUNCHES):
        t0 = time.perf_counter()
        r = subprocess.run([bin, "https://example.com/", "--render", "always", "-f", "text", *extra],
                           capture_output=True, text=True, timeout=120)
        if r.returncode == 0 and "documentation examples" in r.stdout:
            times.append(time.perf_counter() - t0)
    return times


def extraction(label, bin):
    env = {**os.environ, "WEBCLAW_BIN": bin}
    py = str(WORK / "venv-base/bin/python")
    out = ROOT / "results" / label
    (out / "rows.jsonl").unlink(missing_ok=True)
    subprocess.run([py, str(ROOT / "scripts/run.py"), "run", label, EXTRACTION_RUNS, "webclaw-auto", "webclaw-render"],
                   env=env, check=True)
    subprocess.run([py, str(ROOT / "scripts/run.py"), "summary", label], env=env, check=True, capture_output=True)
    rows = {}
    for line in (out / "summary.md").read_text().splitlines():
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) == 8 and cells[0] == "ALL":
            rows[cells[1]] = cells
    return rows


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    pages_file = res / "chrome-ua-pages.json"
    pages_file.write_text(json.dumps(PAGES, indent=1))
    arms = [a for a in ARMS if BASELINE or not a[0].startswith("webclaw-before")]
    rows = []
    for arm, venv, _ in arms:
        rows += [{**row, "arm": arm} for row in run_arm(arm, venv, pages_file)]
    (res / "chrome-ua-rows.jsonl").write_text("".join(json.dumps(slim(r)) + "\n" for r in rows))

    lines = [f"# Presenting as regular Chrome, {label}", "",
             f"{ROUNDS} rounds, a fresh browser per page. Tells come from the httpbin page (headers the browser sent, plus "
             "what the page and a dedicated worker read); the bot check is deviceandbrowserinfo.com's verdict and the "
             "signals it caught; Nasdaq and Bundlephobia count when the page called its data API (the AAPL quote, "
             "the size of react), as the browser's resource timing lists it. Median seconds include the probe's "
             "waits (5 s on the bot check, 2 s on Nasdaq and Bundlephobia).", "",
             "| tool | headless tells | bot check | Nasdaq | Bundlephobia | median s |", "|---|---|---|---:|---:|---:|"]
    for arm, _, name in arms:
        mine = [r for r in rows if r["arm"] == arm]
        found = sorted({t for r in mine if r["page"] == "headers" and r["probe"] for t in tells(r["probe"])})
        probes = [r for r in mine if r["page"] == "headers" and r["probe"]]
        checks = [bot_check(r["probe"]) for r in mine if r["page"] == "bot-check" and r["probe"]]
        checks = [c for c in checks if c]
        verdict = "no result" if not checks else ", ".join(sorted({
            ("bot: " + " ".join(flags)) if bot else "human" for bot, flags in checks}))
        count = lambda page: f"{sum(works(page, r['probe']) for r in mine if r['page'] == page)}/{sum(r['page'] == page for r in mine)}"
        secs = [r["ms"] / 1000 for r in mine if not r["error"]]
        lines.append(f"| {name} | {'; '.join(found) if probes and found else ('none' if probes else 'no result')} | "
                     f"{verdict} | {count('nasdaq')} | {count('bundlephobia')} | "
                     f"{statistics.median(secs) if secs else 0:.2f} |")
    lines.append("")
    example = next((r["probe"] for r in rows if r["arm"] == "webclaw" and r["page"] == "headers" and r["probe"]), None)
    if example:
        lines += ["webclaw's view on this machine: `" + example["ua"] + "`, brands " + ", ".join(example["brands"] or []), ""]

    lines += ["## Launch cost", "", f"One `--render always` render of example.com per process, {LAUNCHES} times.", "",
              "| build | median s | min s | max s |", "|---|---:|---:|---:|"]
    builds = [("webclaw", os.environ["WEBCLAW_BIN"], [])]
    if BASELINE:
        builds += [("webclaw before", BASELINE, []), ("webclaw before, `--solve`", BASELINE, ["--solve"])]
    for name, bin, extra in builds:
        t = launch_cost(bin, extra)
        lines.append(f"| {name} | {statistics.median(t):.2f} | {min(t):.2f} | {max(t):.2f} |" if t else f"| {name} | failed | | |")
    lines.append("")

    lines += ["## Render slice of the extraction benchmark", "",
              f"`run.py` with the webclaw-auto (`--render auto`) and webclaw-render (`--render always`) arms, "
              f"{EXTRACTION_RUNS} runs of every case.", "",
              "| build | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |", "|---|---|---:|---:|---:|---:|---:|---:|"]
    builds = [("webclaw", os.environ["WEBCLAW_BIN"], f"{label}/extraction")]
    if BASELINE:
        builds.append(("webclaw before", BASELINE, f"{label}/extraction-before"))
    if EXTRACTION_RUNS == "0":
        builds = []
    for name, bin, sub in builds:
        for arm, cells in sorted(extraction(sub, bin).items()):
            lines.append(f"| {name} | {arm} | " + " | ".join(cells[2:]) + " |")
    lines.append("")
    (res / "chrome-ua-summary.md").write_text("\n".join(lines))
    print("\n".join(lines))


if __name__ == "__main__":
    main(sys.argv[1])
