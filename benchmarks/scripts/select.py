import json, os, re, statistics, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CORPUS = WORK / "select-corpus"
ARM_VENV = {"webclaw": "base", "scrapling": "scrapling", "crawl4ai": "crawl4ai"}
PAGES = [f"https://books.toscrape.com/catalogue/page-{n}.html" for n in range(1, 51)]


def freeze():
    CORPUS.mkdir(parents=True, exist_ok=True)
    for n, url in enumerate(PAGES, start=1):
        page = CORPUS / f"page-{n:02}.html"
        if page.exists():
            continue
        r = subprocess.run([os.environ["WEBCLAW_BIN"], url, "--raw-html"], capture_output=True, text=True)
        if r.returncode or "product_pod" not in r.stdout:
            sys.exit(f"could not freeze {url} (exit {r.returncode})")
        page.write_text(r.stdout)


def truth(html):
    pods = html.split('<article class="product_pod">')[1:]
    return [{"title": re.search(r'<h3><a href="[^"]*" title="([^"]*)"', pod).group(1).replace("&#39;", "'")
             .replace("&quot;", '"').replace("&amp;", "&"),
             "price": re.search(r'<p class="price_color">([^<]*)</p>', pod).group(1),
             "rating": "star-rating " + re.search(r'<p class="star-rating (\w+)">', pod).group(1)} for pod in pods]


def flat(record):
    return {k: " ".join(v) if isinstance(v, list) else v for k, v in record.items()}


def main(label):
    freeze()
    pages = sorted(CORPUS.glob("page-*.html"))
    expected = [truth(p.read_text()) for p in pages]
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows = []
    for arm, venv in ARM_VENV.items():
        out = WORK / "select-out" / label / f"{arm}.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        py = str(WORK / f"venv-{venv}" / "bin/python")
        subprocess.run([py, str(ROOT / "scripts/select_arms.py"), arm, str(CORPUS), str(out)], check=True)
        got = json.loads(out.read_text())
        correct = sum(flat(rec) == want for page, exp in zip(got["records"], expected) for rec, want in zip(page, exp))
        rows.append({"arm": arm, "records": sum(map(len, got["records"])), "expected": sum(map(len, expected)),
                     "correct": correct, "seconds": got["seconds"], "start_seconds": got.get("start_seconds"),
                     "synthetic_ms": got["synthetic_ms"], "synthetic_values": got["synthetic_values"]})
        print(json.dumps(rows[-1]), flush=True)
    (res / "select-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# Selector benchmark {label}", "",
             f"Each tool reads the same {len(pages)} frozen books.toscrape.com catalogue pages and returns one record "
             "per book (title, price, rating) through its CSS selector API. `correct` counts records equal to the "
             "values parsed from the raw HTML. Seconds are wall time for all pages. Python tools run in one process; "
             "webclaw runs its CLI once per page, and `of which start` is the time the same number of "
             "`webclaw --version` runs takes.", "",
             "`synthetic ms` repeats the test from Scrapling's own benchmarks.py: the text of every `.item` in a "
             "page of 5,000 nested divs, mean wall time over 20 runs. webclaw and Crawl4AI return each element's whole text, descendants included, "
             "so they walk every nested div below each match; Scrapling's `::text` returns only the element's own text "
             "nodes, of which these divs have none. Crawl4AI returns the `class` attribute as a list, joined with spaces "
             "before scoring.", "",
             "| arm | records | correct | seconds | of which start | ms/page | synthetic ms | synthetic values |",
             "|---|---:|---:|---:|---:|---:|---:|---:|"]
    for r in rows:
        start = f"{r['start_seconds']:.2f}" if r["start_seconds"] is not None else "-"
        lines.append(f"| {r['arm']} | {r['records']}/{r['expected']} | {r['correct']} | {r['seconds']:.2f} | {start} | "
                     f"{1000 * r['seconds'] / len(pages):.1f} | {r['synthetic_ms']:.1f} | {r['synthetic_values']} |")
    (res / "select-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
