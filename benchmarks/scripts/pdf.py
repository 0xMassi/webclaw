import json, os, re, statistics, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
URLS = [
    "https://example.com/",
    "https://quotes.toscrape.com/js/",
    "https://books.toscrape.com/",
    "https://en.wikipedia.org/wiki/Web_scraping",
    "https://news.ycombinator.com/",
    "https://www.rust-lang.org/",
    "https://docs.python.org/3/library/asyncio.html",
    "https://github.com/rust-lang/rust",
    "https://developer.mozilla.org/en-US/docs/Web/HTML",
    "https://www.bbc.com/news",
]
PYTHON = {"webclaw": {"cold": "venv-base", "warm": "venv-mcp"},
          "crawl4ai": {"cold": "venv-crawl4ai", "warm": "venv-crawl4ai"}}
UNSUPPORTED = {"scrapling": "no PDF export: its fetchers, sessions and MCP tools return pages, text and screenshots only"}
ONLY = os.environ.get("PDF_ONLY")
WARM_ROUNDS = 2
WORD = re.compile(r"[^\W\d_]{4,}")


def words(text):
    return set(WORD.findall(text.lower()))


def reference(label):
    path = WORK / "pdf-out" / f"reference-{label}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        return json.loads(path.read_text())
    ref = {}
    for url in URLS:
        r = subprocess.run([os.environ["WEBCLAW_BIN"], url, "--render", "always", "-f", "text"],
                           capture_output=True, text=True, timeout=120)
        ref[url] = sorted(words(r.stdout))
    path.write_text(json.dumps(ref))
    return ref


def score(line, ref):
    pdf = json.loads(line)
    if not pdf["file"]:
        return {**pdf, "ok": False}
    path = Path(pdf["file"])
    data = path.read_bytes()
    r = subprocess.run([os.environ["PDF_TEXT_BIN"], str(path)], capture_output=True, text=True, timeout=120)
    path.unlink()
    if not data.startswith(b"%PDF-") or r.returncode != 0:
        return {**pdf, "ok": False, "error": pdf["error"] or f"unreadable PDF: {r.stderr.strip()[-200:]}"}
    pages, _, text = r.stdout.partition("\n")
    want = ref[pdf["url"]]
    found = words(text)
    return {**pdf, "ok": int(pages) >= 1, "kb": len(data) / 1024, "pages": int(pages),
            "recall": sum(w in found for w in want) / len(want) if want else None}


def run(arm, mode, out, ref):
    python = WORK / PYTHON[arm][mode] / "bin/python"
    cmd = [str(python), str(ROOT / "scripts/pdf_arms.py"), arm, mode, str(out)]
    rounds = WARM_ROUNDS if mode == "warm" else 1
    pdfs = []
    for _ in range(rounds):
        r = subprocess.run(cmd + URLS, capture_output=True, text=True, timeout=1800)
        done = [score(l, ref) for l in r.stdout.splitlines() if l.startswith("{")]
        seen = {p["url"] for p in done}
        missing = [u for u in URLS if u not in seen]
        if missing:
            print(f"{arm} {mode}: exit {r.returncode}, {len(missing)} pages missing\n{r.stderr[-2000:]}",
                  file=sys.stderr)
        pdfs += done + [{"url": u, "ok": False, "error": f"no result, exit {r.returncode}"} for u in missing]
    return pdfs


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    ref = reference(label)
    rows, per_page = [], {}
    for mode in ("cold", "warm"):
        for arm in PYTHON:
            if ONLY and ONLY != f"{arm}-{mode}":
                continue
            pdfs = run(arm, mode, WORK / "pdf-out" / f"{arm}-{mode}", ref)
            ok = [p for p in pdfs if p["ok"]]
            ms = sorted(p["ms"] for p in ok) or [0]
            recalls = [p["recall"] for p in ok if p["recall"] is not None]
            row = {"arm": arm, "mode": mode, "pdfs": len(pdfs), "ok": len(ok),
                   "ms": statistics.median(ms), "p95_ms": ms[int(0.95 * (len(ms) - 1))],
                   "kb": statistics.median([p["kb"] for p in ok] or [0]),
                   "pages": statistics.median([p["pages"] for p in ok] or [0]),
                   "recall": statistics.mean(recalls) if recalls else 0,
                   "errors": [p["error"] for p in pdfs if not p["ok"]][:3]}
            rows.append(row)
            print(json.dumps(row), flush=True)
            if mode == "cold":
                per_page[arm] = {p["url"]: p for p in pdfs}
    (res / "pdf-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# PDFs {label}", "",
             f"Each tool prints {len(URLS)} pages to PDF. `cold` starts a fresh browser per page, as one command per "
             "PDF does: `webclaw --save-pdf --print-background`, and Crawl4AI's `pdf=True` with a new crawler. `warm` "
             "keeps one browser open for every page: webclaw through its MCP server's `screenshot` tool with format "
             f"`pdf` and `print_background`, Crawl4AI with one crawler, {WARM_ROUNDS} rounds. Every arm prints US "
             "Letter with backgrounds. `ok` counts files that start with `%PDF-` and that webclaw's PDF reader opens with at least "
             "one page. `recall` is the share of the distinct words (4+ letters) of the page's content, as "
             "`webclaw --render always -f text` reads it once at the start, found in the PDF's text, averaged over "
             "pages. webclaw's PDF reader drops some right-to-left text, so recall is understated on pages that "
             "have it, for every tool alike.", "",
             "| tool | mode | ok | ms (median) | ms (p95) | KB (median) | pages (median) | recall |",
             "|---|---|---:|---:|---:|---:|---:|---:|"]
    for r in rows:
        lines.append(f"| {r['arm']} | {r['mode']} | {r['ok']}/{r['pdfs']} | "
                     f"{r['ms']:.0f} | {r['p95_ms']:.0f} | {r['kb']:.0f} | {r['pages']:g} | {r['recall']:.3f} |")
    lines.append("")
    lines += [f"{arm}: {why}." for arm, why in UNSUPPORTED.items()]
    lines += ["", "Cold run, per page: pages, KB and recall:", "",
              "| page | reference words | " + " | ".join(per_page) + " |", "|---|---:|" + "---|" * len(per_page)]
    for url in URLS:
        cells = []
        for arm in per_page:
            p = per_page[arm].get(url, {})
            recall = "-" if p.get("recall") is None else f"{p['recall']:.2f}"
            cells.append(f"{p['pages']} p, {p['kb']:.0f} KB, {recall}" if p.get("ok") else "fail")
        lines.append(f"| {url} | {len(ref[url])} | " + " | ".join(cells) + " |")
    (res / "pdf-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
