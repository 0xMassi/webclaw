import json, os, statistics, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
EXTRA = {
    "https://books.toscrape.com/": ["A Light in the Attic", "Tipping the Velvet"],
    "https://www.bbc.com/news": [],
    "https://www.theverge.com/": [],
}
HOMEPAGES = ["https://vercel.com", "https://www.notion.com", "https://stripe.com", "https://react.dev",
             "https://nextjs.org/docs", "https://techcrunch.com", "https://www.shopify.com", "https://anthropic.com"]
ARMS = [("webclaw", "none"), ("webclaw", "images,fonts,media"),
        ("scrapling", "off"), ("scrapling", "on"), ("crawl4ai", "off"), ("crawl4ai", "on")]
PYTHON = {"webclaw": "venv-mcp", "scrapling": "venv-scrapling", "crawl4ai": "venv-crawl4ai"}
OPTION = {"webclaw": "`block`", "scrapling": "`disable_resources`", "crawl4ai": "`text_mode`"}
ROUNDS = int(os.environ.get("RENDER_BLOCK_ROUNDS", "3"))
ONLY = os.environ.get("RENDER_BLOCK_ONLY")


def corpus():
    facts = {}
    for line in (ROOT / "cases.jsonl").read_text().splitlines():
        case = json.loads(line)
        if case["category"] == "spa":
            facts[case["url"]] = case["facts"]
    golden = json.loads((ROOT / "facts.json").read_text())["facts"]
    facts.update({url: golden[url] for url in HOMEPAGES})
    facts.update(EXTRA)
    return facts


def run(arm, setting, urls):
    cmd = [str(WORK / PYTHON[arm] / "bin/python"), str(ROOT / "scripts/render_block_arms.py"), arm, setting, *urls]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=3600)
    rows = [json.loads(l) for l in r.stdout.splitlines() if l.startswith("{")]
    seen = {row["url"] for row in rows}
    missing = [u for u in urls if u not in seen]
    if missing:
        print(f"{arm} {setting}: exit {r.returncode}, {len(missing)} pages missing\n{r.stderr[-2000:]}", file=sys.stderr)
    return rows + [{"url": u, "ms": 0, "kb": 0, "text": "", "error": f"no result, exit {r.returncode}"} for u in missing]


def p95(values):
    values = sorted(values)
    return values[int(0.95 * (len(values) - 1))] if values else 0


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    facts = corpus()
    urls = list(facts)
    rows = []
    for rnd in range(ROUNDS):
        for arm, setting in ARMS:
            if ONLY and ONLY != arm:
                continue
            for row in run(arm, setting, urls):
                text = row.pop("text").lower()
                row.update(arm=arm, setting=setting, round=rnd, words=len(text.split()),
                           facts=sum(f.lower() in text for f in facts[row["url"]]), of=len(facts[row["url"]]))
                rows.append(row)
                print(json.dumps(row), flush=True)
    (res / "render-block-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))

    def cell(arm, setting):
        return [r for r in rows if r["arm"] == arm and r["setting"] == setting]

    def page_words(arm, setting):
        out = {}
        for url in urls:
            words = [r["words"] for r in cell(arm, setting) if r["url"] == url and not r["error"]]
            out[url] = statistics.median(words) if words else 0
        return out

    lines = [f"# Rendering with resources blocked {label}", "",
             f"Each tool renders {len(urls)} script-heavy pages in one browser it keeps open, {ROUNDS} rounds, with its "
             "resource blocking off and on, every request going through a local proxy that counts the bytes it carries "
             "(TLS bytes for HTTPS). webclaw renders through its MCP `scrape` tool with render `always` (an HTTP fetch "
             "first, then Chrome with that response); Scrapling through `AsyncDynamicSession(network_idle=True)`; "
             "Crawl4AI through one `AsyncWebCrawler`. `facts` counts the golden facts found in the output; `words` is the "
             "median word count. `lost` lists pages whose median word count fell below 90% of the same tool with "
             "blocking off, or that lost a fact.", "",
             "| tool | option | setting | ok | ms (median) | ms (p95) | KB/page (median) | KB total | facts | words (median) |",
             "|---|---|---|---:|---:|---:|---:|---:|---:|---:|"]
    lost_lines = []
    for arm, setting in ARMS:
        rs = cell(arm, setting)
        if not rs:
            continue
        ok = [r for r in rs if not r["error"] and r["words"] > 0]
        ms = [r["ms"] for r in ok]
        kb = [r["kb"] for r in ok]
        lines.append(f"| {arm} | {OPTION[arm]} | {setting} | {len(ok)}/{len(rs)} | {statistics.median(ms or [0]):.0f} | "
                     f"{p95(ms):.0f} | {statistics.median(kb or [0]):.0f} | {sum(r['kb'] for r in rs):.0f} | "
                     f"{sum(r['facts'] for r in rs)}/{sum(r['of'] for r in rs)} | "
                     f"{statistics.median([r['words'] for r in ok] or [0]):.0f} |")
        base = ARMS[[a for a, _ in ARMS].index(arm)][1]
        if setting == base:
            continue
        before, after = page_words(arm, base), page_words(arm, setting)
        lost = []
        for url in urls:
            f0 = sum(r["facts"] for r in cell(arm, base) if r["url"] == url)
            f1 = sum(r["facts"] for r in rs if r["url"] == url)
            if after[url] < 0.9 * before[url] or f1 < f0:
                lost.append(f"{url} (words {before[url]:.0f} -> {after[url]:.0f}, facts {f0} -> {f1})")
        lost_lines.append(f"- {arm} {setting}: " + ("; ".join(lost) if lost else "none"))
    lines += ["", "Pages that lost content against the same tool with blocking off:", ""] + lost_lines
    lines += ["", "Median ms per page:", "",
              "| page | " + " | ".join(f"{a} {s}" for a, s in ARMS) + " |", "|---|" + "---:|" * len(ARMS)]
    for url in urls:
        cells = []
        for arm, setting in ARMS:
            ms = [r["ms"] for r in cell(arm, setting) if r["url"] == url and not r["error"]]
            cells.append(f"{statistics.median(ms):.0f}" if ms else "fail")
        lines.append(f"| {url} | " + " | ".join(cells) + " |")
    (res / "render-block-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
