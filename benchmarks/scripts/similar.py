import json, os, subprocess, sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CORPUS = WORK / "similar-corpus"
ARM_VENV = {"webclaw": "base", "scrapling": "scrapling", "crawl4ai": "crawl4ai"}
PAGES = [
    ("books", "https://books.toscrape.com/catalogue/page-1.html", "article.product_pod h3 a"),
    ("books-travel", "https://books.toscrape.com/catalogue/category/books/travel_2/index.html",
     "article.product_pod h3 a"),
    ("quotes", "https://quotes.toscrape.com/", "div.quote span.text"),
    ("quotes-love", "https://quotes.toscrape.com/tag/love/", "small.author"),
    ("hn", "https://news.ycombinator.com/", "span.titleline > a"),
    ("hn-newest", "https://news.ycombinator.com/newest", "span.titleline > a"),
    ("lobsters", "https://lobste.rs/", "a.u-url"),
    ("gh-trending", "https://github.com/trending", "article.Box-row h2 a"),
    ("gh-trending-rust", "https://github.com/trending/rust", "article.Box-row h2 a"),
    ("wiki-proglang", "https://en.wikipedia.org/wiki/List_of_programming_languages", "div.div-col li > a"),
    ("wiki-countries", "https://en.wikipedia.org/wiki/List_of_countries_and_dependencies_by_population",
     "table.wikitable > tbody > tr > td:first-child > a"),
    ("countries", "https://www.scrapethissite.com/pages/simple/", "h3.country-name"),
    ("hockey", "https://www.scrapethissite.com/pages/forms/", "tr.team td.name"),
    ("ws-laptops", "https://webscraper.io/test-sites/e-commerce/allinone/computers/laptops", "a.title"),
    ("ws-phones", "https://webscraper.io/test-sites/e-commerce/static/phones/touch", "a.title"),
    ("scrapeme", "https://scrapeme.live/shop/", "h2.woocommerce-loop-product__title"),
    ("scrapingcourse", "https://www.scrapingcourse.com/ecommerce/", "h2.product-name"),
    ("arxiv", "https://arxiv.org/list/cs.AI/recent", "div.list-title"),
    ("devto", "https://dev.to/", "h2.crayons-story__title a"),
    ("cnn-lite", "https://lite.cnn.com/", "li.card--lite a"),
    ("npr-text", "https://text.npr.org/", "a.topic-title"),
    ("bbc", "https://www.bbc.co.uk/news", "[data-testid=promo] h3"),
    ("guardian", "https://www.theguardian.com/international", ".card-headline .headline-text"),
]


def webclaw(*args):
    r = subprocess.run([os.environ["WEBCLAW_BIN"], *args], capture_output=True, text=True)
    if r.returncode:
        sys.exit(f"webclaw {' '.join(args)} failed: {r.stderr}")
    return r.stdout


def freeze():
    CORPUS.mkdir(parents=True, exist_ok=True)
    for name, url, _ in PAGES:
        page = CORPUS / f"{name}.html"
        if not page.exists():
            page.write_text(webclaw(url, "--raw-html"))


def key(text):
    return "".join(text.split())


def score(got, truth):
    if got is None:
        return None
    hit = sum((Counter(map(key, got)) & Counter(map(key, truth))).values())
    return {"precision": hit / len(got) if got else 0.0, "recall": hit / len(truth)}


def mean(values):
    values = [v for v in values if v is not None]
    return sum(values) / len(values) if values else None


def main(label):
    freeze()
    cases = []
    for name, url, gt in PAGES:
        truth = json.loads(webclaw("--file", str(CORPUS / f"{name}.html"), "--select", gt))
        if len(truth) < 3:
            sys.exit(f"{name}: ground truth `{gt}` picks {len(truth)} items")
        cases.append({"name": name, "url": url, "gt": gt, "example": truth[1], "truth": truth})
    cases_file = WORK / "similar-out" / label / "cases.json"
    cases_file.parent.mkdir(parents=True, exist_ok=True)
    cases_file.write_text(json.dumps(cases))
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows, per_page = [], []
    for arm, venv in ARM_VENV.items():
        out = WORK / "similar-out" / label / f"{arm}.json"
        py = str(WORK / f"venv-{venv}" / "bin/python")
        subprocess.run([py, str(ROOT / "scripts/similar_arms.py"), arm, str(CORPUS), str(cases_file), str(out)],
                       check=True)
        got = json.loads(out.read_text())
        if not got.get("supported", True):
            rows.append({"arm": arm, "supported": False, "note": got["note"]})
            print(json.dumps(rows[-1]), flush=True)
            continue
        pages = []
        for case, page in zip(cases, got["pages"]):
            found = page["text_first"]
            pages.append({"arm": arm, "page": case["name"], "items": len(case["truth"]),
                          "text_found": found is not None and key(found) == key(case["example"]),
                          "from_text": score(page["from_text"], case["truth"]),
                          "from_css": score(page["from_css"], case["truth"]),
                          "selector": page.get("selector"),
                          "selector_reuses": page.get("reuse") is not None
                          and Counter(map(key, page["reuse"])) == Counter(map(key, page["from_text"] or [])),
                          "seconds": page["seconds"]})
        per_page += pages
        row = {"arm": arm, "supported": True, "pages": len(pages),
               "text_found": sum(p["text_found"] for p in pages)}
        for mode in ("from_text", "from_css"):
            row[f"{mode}_precision"] = mean([p[mode] and p[mode]["precision"] for p in pages])
            row[f"{mode}_recall"] = mean([p[mode] and p[mode]["recall"] for p in pages])
            row[f"{mode}_exact"] = sum(bool(p[mode]) and p[mode]["precision"] == 1 and p[mode]["recall"] == 1
                                       for p in pages)
        row["selector_reuses"] = sum(p["selector_reuses"] for p in pages)
        row["seconds"] = got["seconds"]
        row["start_seconds"] = got.get("start_seconds")
        rows.append(row)
        print(json.dumps(row), flush=True)
    (res / "similar-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows + per_page))
    lines = [f"# Find-similar benchmark {label}", "",
             f"{len(cases)} list pages frozen into `.work/similar-corpus/`. Each page has a hand-written CSS selector "
             "for its items (titles, names or headlines); the items it picks are the ground truth. The example is the "
             "second item's text. `text found` counts pages where the first element the tool's exact, case-insensitive "
             "text search returns has that text. `from text` runs find-similar on that element; `from css` runs it on the first "
             "item picked by the ground-truth selector. Precision and recall compare the texts of the returned "
             "elements (the example included) with the ground truth, averaged over pages; `exact` counts pages with "
             "both at 1. `reusable selector` counts pages where the selector the tool suggests picks the same "
             "elements again. Seconds are wall time for the `from text` run over all pages; webclaw runs its CLI "
             "once per page, and `of which start` is the time the same number of `webclaw --version` runs takes.", "",
             "| arm | text found | from text P | from text R | from text exact | from css P | from css R | "
             "from css exact | reusable selector | seconds | of which start |",
             "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for r in rows:
        if not r["supported"]:
            lines.append(f"| {r['arm']} | {r['note']} |" + " - |" * 9)
            continue
        start = f"{r['start_seconds']:.2f}" if r["start_seconds"] is not None else "-"
        lines.append(f"| {r['arm']} | {r['text_found']}/{r['pages']} | {r['from_text_precision']:.3f} | "
                     f"{r['from_text_recall']:.3f} | {r['from_text_exact']}/{r['pages']} | "
                     f"{r['from_css_precision']:.3f} | {r['from_css_recall']:.3f} | {r['from_css_exact']}/{r['pages']} | "
                     f"{r['selector_reuses']}/{r['pages']} | {r['seconds']:.2f} | {start} |")
    lines += ["", "Per page (precision/recall from text, then from css):", "",
              "| page | items | " + " | ".join(r["arm"] for r in rows if r["supported"]) + " |",
              "|---|---:|" + "---|" * sum(r["supported"] for r in rows)]

    def cell(p):
        f = lambda s: "-" if not s else f"{s['precision']:.2f}/{s['recall']:.2f}"
        return f"{f(p['from_text'])}, {f(p['from_css'])}"

    for case in cases:
        mine = [p for p in per_page if p["page"] == case["name"]]
        lines.append(f"| {case['name']} | {len(case['truth'])} | " + " | ".join(cell(p) for p in mine) + " |")
    selectors = [p for p in per_page if p["selector"]]
    if selectors:
        lines += ["", "Suggested selectors:", "", "| page | ground truth | suggested |", "|---|---|---|"]
        lines += [f"| {p['page']} | `{c['gt']}` | `{p['selector']}` |"
                  for p in selectors for c in cases if c["name"] == p["page"]]
    (res / "similar-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
