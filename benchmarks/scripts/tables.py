import json, os, re, subprocess, sys
from pathlib import Path

from lxml import html as lhtml

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
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
HELD_PAGES = {
    "de-staedte": "https://de.wikipedia.org/wiki/Liste_der_Gro%C3%9Fst%C3%A4dte_in_Deutschland",
    "fr-ligue1": "https://fr.wikipedia.org/wiki/Championnat_de_France_de_football_2023-2024",
    "ja-prefs": "https://ja.wikipedia.org/wiki/%E9%83%BD%E9%81%93%E5%BA%9C%E7%9C%8C%E3%81%AE%E4%BA%BA%E5%8F%A3%E4%B8%80%E8%A6%A7",
    "es-paises": "https://es.wikipedia.org/wiki/Anexo:Pa%C3%ADses_y_territorios_dependientes_por_poblaci%C3%B3n",
    "f1-results": "https://www.formula1.com/en/results/2024/drivers",
    "skysports": "https://www.skysports.com/premier-league-table/2023",
    "cbs-nfl": "https://www.cbssports.com/nfl/standings/",
    "companiesmc": "https://companiesmarketcap.com/",
    "stockanalysis": "https://stockanalysis.com/stocks/aapl/financials/",
    "boe-rate": "https://www.bankofengland.co.uk/boeapps/database/Bank-Rate.asp",
    "census-qf": "https://www.census.gov/quickfacts/fact/table/US/PST045224",
    "govuk-wage": "https://www.gov.uk/national-minimum-wage-rates",
    "ssa-life": "https://www.ssa.gov/oact/STATS/table4c6.html",
    "php-date": "https://www.php.net/manual/en/function.date.php",
    "sqlite-types": "https://www.sqlite.org/datatype3.html",
    "man-signal": "https://man7.org/linux/man-pages/man7/signal.7.html",
    "timeanddate": "https://www.timeanddate.com/holidays/us/2025",
    "worldometers": "https://www.worldometers.info/world-population/population-by-country/",
    "usinfl": "https://www.usinflationcalculator.com/inflation/historical-inflation-rates/",
}
SETS = {"corpus": (WORK / "tables-corpus", PAGES, "tables-labels.json"),
        "held": (WORK / "tables-held", HELD_PAGES, "tables-held-labels.json")}
BLOCK = {"address", "article", "aside", "blockquote", "caption", "dd", "div", "dl", "dt", "figcaption", "figure",
         "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "li", "main", "nav", "ol", "option",
         "p", "pre", "section", "table", "td", "th", "tr", "ul"}


def freeze(corpus, pages):
    corpus.mkdir(parents=True, exist_ok=True)
    for slug, url in pages.items():
        page = corpus / f"{slug}.html"
        if page.exists():
            continue
        r = subprocess.run([os.environ["WEBCLAW_BIN"], url, "--raw-html"], capture_output=True, text=True)
        if r.returncode or "<table" not in r.stdout:
            sys.exit(f"could not freeze {url} (exit {r.returncode})")
        page.write_text(r.stdout)
    (corpus / "urls.json").write_text(json.dumps(pages, indent=1))


def browser_truth(name, corpus):
    out = WORK / f"tables-truth-{name}.json"
    if not out.exists():
        subprocess.run([str(WORK / "venv-crawl4ai/bin/python"), str(ROOT / "scripts/tables_truth.py"),
                        str(corpus), str(out)], check=True)
    return {slug: [split(t) for t in tables] for slug, tables in json.loads(out.read_text()).items()}


def split(t):
    grid, width = t["grid"], len(t["grid"][0]) if t["grid"] else 0
    k = 0
    while k < len(grid) and (t["head"][k] or (width > 1 and t["single"][k] >= width)):
        k += 1
    titles = [r for r in range(k) if width > 1 and t["single"][r] >= width]
    return {"head": [grid[r] for r in range(k) if r not in titles],
            "body": [row for row in grid[k:] if any(row)]}


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


def number(value):
    m = re.match(r"\s*\+?(\d+)", value or "")
    return int(m.group(1)) if m else None


def reference(table):
    trs = [tr for tr in table.iter("tr") if next(tr.iterancestors("table")) is table
           and not hidden(tr) and not hidden(tr.getparent())]
    trs.sort(key=lambda tr: {"thead": 0, "tfoot": 2}.get(tr.getparent().tag, 1))
    slots, single, flags = {}, [], []
    for r, tr in enumerate(trs):
        cells = [c for c in tr if c.tag in ("td", "th") and not hidden(c)]
        single.append(min(number(cells[0].get("colspan")) or 1, 1000) if len(cells) == 1 else 0)
        flags.append(bool(cells) and (tr.getparent().tag == "thead" or all(c.tag == "th" for c in cells)))
        left = sum(1 for x in trs[r:] if x.getparent() is tr.getparent())
        c = 0
        for cell in cells:
            while (r, c) in slots:
                c += 1
            cs = min(number(cell.get("colspan")) or 1, 1000)
            rs = number(cell.get("rowspan"))
            rs = left if rs == 0 else min(rs or 1, left)
            t = text(cell)
            for dr in range(rs):
                for dc in range(cs):
                    slots[(r + dr, c + dc)] = t
            c += cs
    width = max((c + 1 for _, c in slots), default=0)
    grid = [[slots.get((r, c), "") for c in range(width)] for r in range(len(trs))]
    return split({"grid": grid, "head": flags, "single": single})


def fingerprint(rows):
    return {norm(x) for row in rows for x in row if norm(x)}


def align(body, rows):
    prev = [0] * (len(rows) + 1)
    for want in body:
        cur = [0] * (len(rows) + 1)
        for j, have in enumerate(rows, start=1):
            same = sum(1 for c, w in enumerate(want) if w and c < len(have) and have[c] == w)
            cur[j] = max(prev[j - 1] + same, prev[j], cur[j - 1])
        prev = cur
    return prev[-1]


def header_ok(head, header, c):
    levels = list(dict.fromkeys(row[c] for row in head if c < len(row) and row[c]))
    return set(levels) <= set(header.split(" / ")) or header in {" ".join(levels), " / ".join(levels)}


def score_table(ref, got):
    rows = [[norm(x) for x in row] for row in got.get("rows") or [] if any(norm(x) for x in row)]
    total = sum(1 for row in ref["body"] for w in row if w)
    headers = [norm(h) for h in got.get("headers") or []]
    hcols = [c for c in range(len(ref["head"][0]) if ref["head"] else 0) if any(row[c] for row in ref["head"])]
    hok = sum(1 for c in hcols if c < len(headers) and header_ok(ref["head"], headers[c], c))
    return align(ref["body"], rows), total, hok, len(hcols)


def score_page(label, refs, got):
    data, either = set(label["data"]), set(label.get("either", []))
    prints = [fingerprint(r["head"] + r["body"]) for r in refs]
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
    for i in sorted(data):
        miss = (0, sum(1 for row in refs[i]["body"] for w in row if w), 0,
                sum(1 for c in range(len(refs[i]["head"][0]) if refs[i]["head"] else 0)
                    if any(row[c] for row in refs[i]["head"])))
        ok, total, hok, htotal = score_table(refs[i], got[match[i]]) if i in match else miss
        s["tp"] += i in match
        s["cells"] += ok
        s["cells_total"] += total
        s["head"] += hok
        s["head_total"] += htotal
        s["exact"] += i in match and ok == total and hok == htotal
    s["ignored"] = sum(1 for i in either if i in match)
    return s


def run_arms(label, name, corpus):
    out = {}
    for arm, venv in ARM_VENV.items():
        path = WORK / "tables-out" / label / f"{name}-{arm}.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        py = str(WORK / f"venv-{venv}" / "bin/python")
        subprocess.run([py, str(ROOT / "scripts/tables_arms.py"), arm, str(corpus), str(path)], check=True)
        out[arm] = json.loads(path.read_text())
    return out


def total(name, truth, refs, labels, arms):
    rows = []
    for arm, got in arms.items():
        per = {slug: score_page(labels[slug], refs[slug], got["tables"][slug]) for slug in labels}
        t = {k: sum(p[k] for p in per.values()) for k in next(iter(per.values()))}
        counted = t["found"] - t["ignored"]
        rows.append({"set": name, "truth": truth, "arm": arm, **t,
                     "precision": t["tp"] / counted if counted else 0.0, "recall": t["tp"] / t["expected"],
                     "truncated": sum(x.get("truncated", False) for ts in got["tables"].values() for x in ts),
                     "seconds": got["seconds"], "start_seconds": got.get("start_seconds"), "pages": per})
    return rows


def table(rows, timing):
    lines = ["| arm | returned | data tables found | precision | recall | body cells | header cells | exact |"
             + (" seconds | of which start |" if timing else ""),
             "|---|---:|---:|---:|---:|---:|---:|---:|" + ("---:|---:|" if timing else "")]
    for r in rows:
        line = (f"| {r['arm']} | {r['found']} | {r['tp']}/{r['expected']} | {r['precision']:.2f} | {r['recall']:.2f} | "
                f"{r['cells']}/{r['cells_total']} ({r['cells'] / r['cells_total']:.1%}) | "
                f"{r['head']}/{r['head_total']} | {r['exact']} |")
        if timing:
            start = f"{r['start_seconds']:.2f}" if r["start_seconds"] is not None else "-"
            line += f" {r['seconds']:.2f} | {start} |"
        lines.append(line)
    return lines


def main(label):
    rows = []
    for name, (corpus, pages, labels_file) in SETS.items():
        freeze(corpus, pages)
        labels = json.loads((ROOT / labels_file).read_text())
        arms = run_arms(label, name, corpus)
        chrome = browser_truth(name, corpus)
        if name == "corpus":
            parser = lhtml.HTMLParser(encoding="utf-8")
            refs = {slug: [reference(t) for t in lhtml.fromstring((corpus / f"{slug}.html").read_bytes(),
                                                                   parser=parser).iter("table")] for slug in pages}
            rows += total(name, "lxml", refs, labels, arms)
        styled = {slug: {"data": [i for i in lab["data"] if i not in lab.get("styled_hidden", [])],
                         "either": lab.get("either", []) + lab.get("styled_hidden", [])}
                  for slug, lab in labels.items()}
        rows += total(name, "chrome", chrome, styled, arms)
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    (res / "tables-rows.jsonl").write_text("".join(json.dumps({k: v for k, v in r.items() if k != "pages"}) + "\n"
                                                   for r in rows))
    (res / "tables-pages.json").write_text(json.dumps({f"{r['set']}-{r['truth']}-{r['arm']}": r["pages"]
                                                       for r in rows}, indent=1))
    pick = lambda s, t: [r for r in rows if r["set"] == s and r["truth"] == t]
    lines = [f"# Table benchmark {label}", "",
             f"Each tool reads the same frozen pages and returns the data tables it finds: {len(PAGES)} pages in the "
             f"corpus and {len(HELD_PAGES)} held-out pages that were not looked at while writing webclaw's rules. "
             "Every table is labelled by hand in `tables-labels.json` and `tables-held-labels.json` as a data table, "
             "a layout table, or neither (infoboxes and key/value boxes), which counts neither way. A returned table "
             "is matched to the page table whose cell texts it overlaps most. Precision is matched data tables over "
             "returned tables; recall is matched data tables over expected ones. `body cells` counts non-empty body "
             "cells equal to the truth after whitespace collapsing, rows aligned in order; `header cells` counts "
             "columns whose header holds each header row's text, joined with a space or ` / `. `exact` counts tables "
             "with every body and header cell right.", "",
             "Two truths. Chrome: the page loaded in headless Chrome with scripts off and stylesheets on, each "
             "visible cell's `innerText` laid out on the grid by its spans; tables a stylesheet hides count neither "
             "way. lxml: the raw HTML laid out the same way with lxml, hidden meaning `hidden`, inline "
             "`display:none` or `<template>`. In both a spanning cell fills every slot it covers, and a header row "
             "that is one cell across the table is a title, not column names.", "",
             f"## Corpus against Chrome ({len(PAGES)} pages)", "",
             "Seconds are wall time for all pages; webclaw runs its CLI once per page, and `of which start` is the "
             "time the same number of `webclaw --version` runs takes. Crawl4AI's time is its whole scraping pass, "
             "which is how its users get tables.", "", *table(pick("corpus", "chrome"), True), "",
             f"## Held-out pages against Chrome ({len(HELD_PAGES)} pages)", "", *table(pick("held", "chrome"), False), "",
             "## Corpus against the lxml reference", "", *table(pick("corpus", "lxml"), False), "",
             "Tables cut short by webclaw's size limits: "
             + ", ".join(f"{r['set']} {r['truncated']}" for r in rows if r["arm"] == "webclaw" and r["truth"] == "chrome")
             + "."]
    (res / "tables-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
