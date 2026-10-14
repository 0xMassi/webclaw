import json, os, re, subprocess, sys
from pathlib import Path

from lxml import html as lhtml

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CORPUS = WORK / "tables-corpus"
LABELS = json.loads((ROOT / "tables-labels.json").read_text())
ARM_VENV = {"webclaw": "base", "crawl4ai": "crawl4ai", "scrapling": "scrapling"}
PAGES = {
    "wiki-population": "https://en.wikipedia.org/wiki/List_of_countries_and_dependencies_by_population",
    "wiki-gdp": "https://en.wikipedia.org/wiki/List_of_countries_by_GDP_(nominal)",
    "wiki-python": "https://en.wikipedia.org/wiki/Python_(programming_language)",
    "wiki-comparison": "https://en.wikipedia.org/wiki/Comparison_of_programming_languages",
    "wiki-worldcup": "https://en.wikipedia.org/wiki/2022_FIFA_World_Cup",
    "wiki-buildings": "https://en.wikipedia.org/wiki/List_of_tallest_buildings",
    "wiki-nobel": "https://en.wikipedia.org/wiki/List_of_Nobel_laureates_in_Physics",
    "wiki-sp500": "https://en.wikipedia.org/wiki/List_of_S%26P_500_companies",
    "wiki-olympics": "https://en.wikipedia.org/wiki/2024_Summer_Olympics_medal_table",
    "wiki-premier": "https://en.wikipedia.org/wiki/2023%E2%80%9324_Premier_League",
    "irs-brackets": "https://www.irs.gov/filing/federal-income-tax-rates-and-brackets",
    "ssa-cbb": "https://www.ssa.gov/oact/cola/cbb.html",
    "fed-h15": "https://www.federalreserve.gov/releases/h15/",
    "bls-cpi": "https://www.bls.gov/news.release/cpi.t01.htm",
    "eia-gas": "https://www.eia.gov/dnav/pet/pet_pri_gnd_dcus_nus_w.htm",
    "bbref-standings": "https://www.basketball-reference.com/leagues/NBA_2024_standings.html",
    "worldfootball": "https://www.worldfootball.net/schedule/eng-premier-league-2023-2024-spieltag/38/",
    "mdn-table": "https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/table",
    "py-stdtypes": "https://docs.python.org/3/library/stdtypes.html",
    "pg-numeric": "https://www.postgresql.org/docs/current/datatype-numeric.html",
    "nginx-core": "https://nginx.org/en/docs/http/ngx_http_core_module.html",
    "docker-run": "https://docs.docker.com/reference/cli/docker/container/run/",
    "w3s-tables": "https://www.w3schools.com/html/html_tables.asp",
    "rust-operators": "https://doc.rust-lang.org/book/appendix-02-operators.html",
    "xrates": "https://www.x-rates.com/table/?from=USD&amount=1",
    "multpl": "https://www.multpl.com/s-p-500-pe-ratio/table/by-month",
    "ecb-rates": "https://www.ecb.europa.eu/stats/policy_and_exchange_rates/euro_reference_exchange_rates/html/index.en.html",
    "treasury-yields": "https://home.treasury.gov/resource-center/data-chart-center/interest-rates/TextView?type=daily_treasury_yield_curve&field_tdr_date_value_month=202609",
    "slickcharts": "https://www.slickcharts.com/sp500",
    "hn-front": "https://news.ycombinator.com/",
    "hn-item": "https://news.ycombinator.com/item?id=8863",
    "pg-articles": "https://www.paulgraham.com/articles.html",
    "berkshire": "https://www.berkshirehathaway.com/",
    "spacejam": "https://www.spacejam.com/1996/",
    "ddg-lite": "https://lite.duckduckgo.com/lite/?q=rust+language",
}
BLOCK = {"address", "article", "aside", "blockquote", "caption", "dd", "div", "dl", "dt", "figcaption", "figure",
         "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "li", "main", "nav", "ol", "p", "pre",
         "section", "table", "td", "th", "tr", "ul"}


def freeze():
    CORPUS.mkdir(parents=True, exist_ok=True)
    for slug, url in PAGES.items():
        page = CORPUS / f"{slug}.html"
        if page.exists():
            continue
        r = subprocess.run([os.environ["WEBCLAW_BIN"], url, "--raw-html"], capture_output=True, text=True)
        if r.returncode or "<table" not in r.stdout:
            sys.exit(f"could not freeze {url} (exit {r.returncode})")
        page.write_text(r.stdout)
    (CORPUS / "urls.json").write_text(json.dumps(PAGES, indent=1))


def norm(s):
    return " ".join(str(s or "").split())


def hidden(el):
    return (el.tag in ("template", "script", "style", "noscript") or el.get("hidden") is not None
            or re.search(r"display\s*:\s*none", el.get("style") or "", re.I) is not None)


def text(el):
    out = []

    def walk(e):
        if hidden(e):
            return
        if e.tag == "br":
            out.append(" ")
        out.append(" " if e.tag in BLOCK else "")
        out.append(e.text or "")
        for c in e:
            if isinstance(c.tag, str):
                walk(c)
            out.append(c.tail or "")
        out.append(" " if e.tag in BLOCK else "")

    walk(el)
    return norm("".join(out))


def span(value, limit):
    try:
        n = int(value)
    except (TypeError, ValueError):
        return 1
    return limit if n == 0 else max(1, min(n, limit))


def reference(table):
    trs = [tr for tr in table.iter("tr") if next(tr.iterancestors("table")) is table
           and not hidden(tr) and not hidden(tr.getparent())]
    slots, heads, single = {}, 0, []
    for r, tr in enumerate(trs):
        cells = [c for c in tr if c.tag in ("td", "th") and not hidden(c)]
        single.append(span(cells[0].get("colspan"), 1000) if len(cells) == 1 else 0)
        if heads == r and cells and (tr.getparent().tag == "thead" or all(c.tag == "th" for c in cells)):
            heads += 1
        c = 0
        for cell in cells:
            while (r, c) in slots:
                c += 1
            cs, rs = span(cell.get("colspan"), 1000), span(cell.get("rowspan"), len(trs) - r)
            t = text(cell)
            for dr in range(rs):
                for dc in range(cs):
                    slots[(r + dr, c + dc)] = t
            c += cs
    width = max((c + 1 for _, c in slots), default=0)
    grid = [[slots.get((r, c), "") for c in range(width)] for r in range(len(trs))]
    titles = 0
    while titles < heads and width > 1 and single[titles] >= width:
        titles += 1
    return {"titles": grid[:titles], "head": grid[titles:heads], "body": [row for row in grid[heads:] if any(row)]}


def fingerprint(rows):
    return {norm(x) for row in rows for x in row if norm(x)}


def accept(head, c):
    levels = list(dict.fromkeys(row[c] for row in head if row[c]))
    return set(levels) | {" ".join(levels), " / ".join(levels)}


def score_table(ref, got):
    rows = [[norm(x) for x in row] for row in got.get("rows") or [] if any(norm(x) for x in row)]
    best = 0
    for off in range(-3, 4):
        ok = 0
        for i, want in enumerate(ref["body"]):
            j = i + off
            if 0 <= j < len(rows):
                ok += sum(1 for c, w in enumerate(want) if w and c < len(rows[j]) and rows[j][c] == w)
        best = max(best, ok)
    total = sum(1 for row in ref["body"] for w in row if w)
    headers = [norm(h) for h in got.get("headers") or []]
    width = len(ref["head"][0]) if ref["head"] else 0
    hcols = [c for c in range(width) if any(row[c] for row in ref["head"])]
    hok = sum(1 for c in hcols if c < len(headers) and headers[c] in accept(ref["head"], c))
    return best, total, hok, len(hcols)


def score_page(slug, refs, got):
    label = LABELS[slug]
    data, either = set(label["data"]), set(label.get("either", []))
    prints = [fingerprint(r["titles"] + r["head"] + r["body"]) for r in refs]
    pairs = []
    for a, table in enumerate(got):
        fp = fingerprint([table.get("headers") or []] + (table.get("rows") or []))
        for i, p in enumerate(prints):
            if fp and p:
                j = len(fp & p) / len(fp | p)
                if j >= 0.3:
                    pairs.append((j, a, i))
    used_a, used_i, match = set(), set(), {}
    for j, a, i in sorted(pairs, reverse=True):
        if a not in used_a and i not in used_i:
            used_a.add(a), used_i.add(i)
            match[i] = a
    s = {"found": len(got), "expected": len(data), "tp": 0, "ignored": 0, "cells": 0, "cells_total": 0,
         "head": 0, "head_total": 0, "exact": 0}
    for i in range(len(refs)):
        if i in data:
            ok, total, hok, htotal = score_table(refs[i], got[match[i]]) if i in match else (0, 0, 0, 0)
            total = total or sum(1 for row in refs[i]["body"] for w in row if w)
            htotal = htotal or len([c for c in range(len(refs[i]["head"][0]) if refs[i]["head"] else 0)
                                    if any(row[c] for row in refs[i]["head"])])
            s["tp"] += i in match
            s["cells"] += ok
            s["cells_total"] += total
            s["head"] += hok
            s["head_total"] += htotal
            s["exact"] += i in match and ok == total and hok == htotal
        elif i in either and i in match:
            s["ignored"] += 1
    return s


def main(label):
    freeze()
    urls = json.loads((CORPUS / "urls.json").read_text())
    parser = lhtml.HTMLParser(encoding="utf-8")
    refs = {slug: [reference(t) for t in lhtml.fromstring((CORPUS / f"{slug}.html").read_bytes(), parser=parser)
                   .iter("table")] for slug in urls}
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows, pages = [], {}
    for arm, venv in ARM_VENV.items():
        out = WORK / "tables-out" / label / f"{arm}.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        py = str(WORK / f"venv-{venv}" / "bin/python")
        subprocess.run([py, str(ROOT / "scripts/tables_arms.py"), arm, str(CORPUS), str(out)], check=True)
        got = json.loads(out.read_text())
        per = {slug: score_page(slug, refs[slug], got["tables"][slug]) for slug in urls}
        pages[arm] = per
        t = {k: sum(p[k] for p in per.values()) for k in next(iter(per.values()))}
        counted = t["found"] - t["ignored"]
        rows.append({"arm": arm, **t, "precision": t["tp"] / counted if counted else 0.0,
                     "recall": t["tp"] / t["expected"], "seconds": got["seconds"],
                     "start_seconds": got.get("start_seconds")})
        print(json.dumps(rows[-1]), flush=True)
    (res / "tables-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    (res / "tables-pages.json").write_text(json.dumps(pages, indent=1))
    n = len(urls)
    lines = [f"# Table benchmark {label}", "",
             f"Each tool reads the same {n} frozen pages and returns the data tables it finds. "
             f"{rows[0]['expected']} tables are labelled as data tables by hand; the rest are layout tables, "
             "except infoboxes and similar key/value boxes, which count neither way. Cell text is what a browser shows: "
             "hidden elements are left out, and a cell spanning several rows or columns fills each slot it covers. "
             "Header rows are the rows of `<thead>` and the leading rows made only of `<th>`; a header row that is "
             "one cell spanning the whole table is a title, not column names, and columns named only by a title "
             "are not scored. A returned table is matched "
             "to the page table whose cell texts it overlaps most. Precision is matched data tables over returned "
             "tables; recall is matched data tables over expected ones. `body cells` counts non-empty body cells "
             "equal to the reference after whitespace collapsing, rows aligned at the best offset; `header cells` "
             "counts columns whose header equals one header row's text, or the levels joined by a space or ` / `. "
             "`exact` counts tables with every body and header cell right. Seconds are wall time for all pages; "
             "webclaw runs its CLI once per page, and `of which start` is the time the same number of "
             "`webclaw --version` runs takes.", "",
             "| arm | returned | data tables found | precision | recall | body cells | header cells | exact | seconds | of which start |",
             "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for r in rows:
        start = f"{r['start_seconds']:.2f}" if r["start_seconds"] is not None else "-"
        lines.append(f"| {r['arm']} | {r['found']} | {r['tp']}/{r['expected']} | {r['precision']:.2f} | "
                     f"{r['recall']:.2f} | {r['cells']}/{r['cells_total']} ({r['cells'] / r['cells_total']:.1%}) | "
                     f"{r['head']}/{r['head_total']} | {r['exact']} | {r['seconds']:.2f} | {start} |")
    (res / "tables-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
