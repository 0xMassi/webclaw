"""Question-guided crawl benchmark, driven by ../query-crawl.sh. Runs under .work/venv-base.

usage: query_crawl.py <label>              crawl, then score
       query_crawl.py <label> --rescore    score the saved outputs again
Each question names the page(s) that answer it. The questions come in three sets, scored apart: `tuned`
(../query-crawl.jsonl, the set the scoring weights were chosen on), `held-out` (../query-crawl-held.jsonl, four
other documentation sites, written by a reviewer after the weights were fixed) and `intl` (../query-crawl-intl.jsonl,
questions in Japanese, French, Spanish and Chinese on the translated Python docs). Every arm crawls from the site's
seed with the same page budget, depth, concurrency and path prefix, robots.txt obeyed. Breadth-first arms ignore
the question, so they crawl each site once. QC_ARMS=a,b and QC_SETS=a,b run a subset; QC_CACHE=1 lets webclaw
reuse its cache.
"""
import json, os, statistics, subprocess, sys, time
from collections import defaultdict
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
SITES = {
    "docs.python.org": ("https://docs.python.org/3/", "/3/"),
    "www.postgresql.org": ("https://www.postgresql.org/docs/current/", "/docs/current/"),
    "developer.mozilla.org": ("https://developer.mozilla.org/en-US/docs/Web/HTTP", "/en-US/docs/Web/HTTP"),
    "quotes.toscrape.com": ("https://quotes.toscrape.com/", "/"),
    "kubernetes.io": ("https://kubernetes.io/docs/concepts/", "/docs/concepts/"),
    "docs.djangoproject.com": ("https://docs.djangoproject.com/en/6.1/", "/en/6.1/"),
    "sqlite.org": ("https://sqlite.org/docs.html", "/"),
    "go.dev": ("https://go.dev/doc/", "/doc/"),
    "py-ja": ("https://docs.python.org/ja/3/", "/ja/3/"),
    "py-fr": ("https://docs.python.org/fr/3/", "/fr/3/"),
    "py-es": ("https://docs.python.org/es/3/", "/es/3/"),
    "py-zh": ("https://docs.python.org/zh-cn/3/", "/zh-cn/3/"),
}
SETS = {"tuned": "query-crawl.jsonl", "held-out": "query-crawl-held.jsonl", "intl": "query-crawl-intl.jsonl"}
ARMS = {"webclaw-bfs": ("base", False), "webclaw-query": ("base", True), "scrapling-bfs": ("scrapling", False),
        "crawl4ai-bfs": ("crawl4ai", False), "crawl4ai-best-first": ("crawl4ai", True),
        "crawl4ai-adaptive": ("crawl4ai", True)}
BUDGET, DEPTH, CONCURRENCY, CUTS = 50, 5, 4, (10, 25, 50)
TOP = 3


def norm(url):
    u = urlparse(url)
    return f"{u.netloc.lower()}{u.path.rstrip('/') or '/'}{'?' + u.query if u.query else ''}"


def verify(questions):
    bad = []
    for q in questions:
        for url in q["answer"]:
            cache = ["--cache"] if os.environ.get("QC_CACHE") else []
            r = subprocess.run([os.environ["WEBCLAW_BIN"], url] + cache, capture_output=True, text=True)
            if r.returncode or q["snippet"].lower() not in r.stdout.lower():
                bad.append(f"{url} lacks {q['snippet']!r}")
    if bad:
        sys.exit("ground truth no longer holds, not scoring:\n" + "\n".join(bad))


def run(arm, site, out, question=None):
    venv, _ = ARMS[arm]
    seed, prefix = SITES[site]
    cmd = [str(WORK / f"venv-{venv}" / "bin/python"), str(ROOT / "scripts/query_crawl_arms.py"), arm, seed, prefix,
           str(BUDGET), str(DEPTH), str(CONCURRENCY), str(out)] + ([question] if question else [])
    out.unlink(missing_ok=True)
    start = time.time()
    r = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, timeout=1800)
    if r.returncode or not out.exists():
        sys.exit(f"{arm} on {site} exited {r.returncode}: {r.stderr[-2000:]}")
    return round(time.time() - start, 2)


def score(q, arm, crawl):
    want = {norm(u) for u in q["answer"]}
    order = [norm(p["url"]) for p in crawl["pages"]][:BUDGET]
    hit = next((i + 1 for i, u in enumerate(order) if u in want), None)
    ranked = [norm(u) for u in crawl["ranked"]][:TOP]
    return {"set": q["set"], "site": q["site"], "q": q["q"], "arm": arm, "hit": hit, "pages": len(order),
            "hit_t": crawl["pages"][hit - 1].get("t") if hit else None,
            "top": any(u in want for u in ranked), "seconds": crawl["seconds"], "previews": crawl["previews"]}


def main(label, rescore):
    sets = [s for s in SETS if s in os.environ.get("QC_SETS", ",".join(SETS)).split(",")]
    questions = [{**json.loads(l), "set": s} for s in sets for l in open(ROOT / SETS[s])]
    arms = [a for a in ARMS if a in os.environ.get("QC_ARMS", ",".join(ARMS)).split(",")]
    res, out_dir = ROOT / "results" / label, WORK / "query-crawl-out" / label
    res.mkdir(parents=True, exist_ok=True)
    out_dir.mkdir(parents=True, exist_ok=True)
    if not rescore:
        verify(questions)
    rows = []
    for arm in arms:
        guided = ARMS[arm][1]
        for i, q in enumerate(questions):
            key = f"{arm}-{q['set']}-{q['site']}-{i if guided else 'all'}"
            out = out_dir / f"{key}.json"
            if not rescore and (guided or not any(r["arm"] == arm and r["site"] == q["site"] and r["set"] == q["set"]
                                                   for r in rows)):
                run(arm, q["site"], out, q["q"] if guided else None)
            row = score(q, arm, json.loads(out.read_text()))
            rows.append(row)
            print(json.dumps(row), flush=True)
    (res / "query-crawl-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    write_summary(label, rows, arms, sets)


def table(rows, arms):
    lines = [f"| arm | questions | answer by page {CUTS[0]} | by {CUTS[1]} | by {CUTS[2]} | median pages to answer "
             f"| answer in output top {TOP} | median seconds to answer | median seconds per crawl | pages fetched |",
             "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for arm in arms:
        rs = [r for r in rows if r["arm"] == arm]
        if not rs:
            continue
        n = len(rs)
        found = [sum(1 for r in rs if r["hit"] and r["hit"] <= c) for c in CUTS]
        to_answer = statistics.median(r["hit"] or BUDGET + 1 for r in rs)
        times = [r["hit_t"] for r in rs if r["hit_t"] is not None]
        crawls = {(r["site"], r["seconds"], r["pages"]) for r in rs} if not ARMS[arm][1] else None
        secs = statistics.median(r["seconds"] for r in rs) if crawls is None else statistics.median(c[1] for c in crawls)
        pages = sum(r["pages"] for r in rs) / n
        lines.append(f"| {arm} | {n} | {found[0]} | {found[1]} | {found[2]} | "
                     f"{to_answer:.0f}{'+' if to_answer > BUDGET else ''} | {sum(r['top'] for r in rs)} | "
                     f"{statistics.median(times) if times else float('nan'):.1f} | {secs:.1f} | {pages:.0f} |")
    return lines


def write_summary(label, rows, arms, sets):
    lines = [f"# Question-guided crawl benchmark {label}", "",
             f"Each question names the page that answers it. Every arm crawls from the site's start page with a "
             f"budget of {BUDGET} pages, depth {DEPTH}, concurrency {CONCURRENCY}, the same path prefix, robots.txt "
             f"obeyed. `answer by page N` counts questions whose answer page was among the first N pages the tool "
             f"fetched. `median pages to answer` counts a miss as {BUDGET + 1}. `answer in output top {TOP}` counts "
             f"questions whose answer page is in the first {TOP} pages of the tool's own ranking (breadth-first arms: "
             f"crawl order). `median seconds to answer` covers found answers only.", "",
             "Question sets: `tuned` is the set the scoring weights were chosen on, `held-out` was written after "
             "they were fixed, on four other sites, and `intl` asks in Japanese, French, Spanish and Chinese."]
    for name in sets:
        set_rows = [r for r in rows if r["set"] == name]
        lines += ["", f"## {name}: all sites", ""] + table(set_rows, arms)
        for site in SITES:
            site_rows = [r for r in set_rows if r["site"] == site]
            if site_rows:
                lines += ["", f"### {name}: {site}", ""] + table(site_rows, arms)
    previews = [r["previews"] for r in rows if r["arm"] == "crawl4ai-adaptive"]
    if previews:
        lines += ["", f"crawl4ai-adaptive also requested the `<head>` of up to {statistics.median(previews):.0f} "
                  "linked pages per question (median) to preview them, on top of the pages above."]
    (ROOT / "results" / label / "query-crawl-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main(sys.argv[1], len(sys.argv) > 2 and sys.argv[2] == "--rescore")
