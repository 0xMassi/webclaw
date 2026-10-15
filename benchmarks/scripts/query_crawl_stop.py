"""Replays webclaw's question-guided crawls under a stop rule, driven by ../query-crawl.sh.

usage: query_crawl_stop.py <label>
Reads the `webclaw-query` crawls of query_crawl.py (pages in fetch order, each with its relevance) and replays
the rule "stop once the best page so far rates at least F and the K pages after it rate no higher" for each K
and F. A stop after page k keeps the pages already in flight, so the crawl fetches k + concurrency - 1 pages.
`--stop-when-answered` ships K=8, F=0.8: on the tuned set, over three runs, the cell with the fewest pages among
those that kept every answer the full crawl found, with all eight neighbouring cells keeping every answer too.
"""
import json, os, sys
import query_crawl as qc

KS = range(3, 21)
FLOORS = [f / 10 for f in range(3, 11)]
SHIPPED = (8, 0.8)


def crawls(label):
    out = []
    for q in qc.load_questions()[1]:
        path = qc.WORK / "query-crawl-out" / label / f"webclaw-query-{q['set']}-{q['site']}-{q['n']}.json"
        pages = json.loads(path.read_text())["pages"][:qc.BUDGET]
        want = {qc.norm(u) for u in q["answer"]}
        hit = next((n + 1 for n, p in enumerate(pages) if qc.norm(p["url"]) in want), None)
        out.append({"set": q["set"], "rels": [p.get("relevance") or 0 for p in pages], "hit": hit,
                    "ans": [qc.norm(p["url"]) in want for p in pages]})
    return out


def replay(c, k_quiet, floor):
    best, since, n = 0.0, 0, len(c["rels"])
    for k, r in enumerate(c["rels"], 1):
        if r > best:
            best, since = r, 0
        else:
            since += 1
        if best >= floor and since >= k_quiet:
            n = min(k + qc.CONCURRENCY - 1, n)
            break
    top = sorted(range(n), key=lambda i: -c["rels"][i])[:qc.TOP]
    return c["hit"] is not None and c["hit"] <= n, n, any(c["ans"][i] for i in top)


def cell(cs, k_quiet, floor):
    runs = [replay(c, k_quiet, floor) for c in cs]
    return {"kept": sum(r[0] for r in runs), "found": sum(c["hit"] is not None for c in cs),
            "pages": sum(r[1] for r in runs) / len(runs), "top": sum(r[2] for r in runs)}


def main(label):
    cs = crawls(label)
    sets = sorted({c["set"] for c in cs}, key=list(qc.SETS).index)
    grids = {s: {(k, f): cell([c for c in cs if c["set"] == s], k, f) for k in KS for f in FLOORS} for s in sets}
    lines = [f"# Stop rule replay {label}", "",
             "Replays the `webclaw-query` crawls with the rule \"stop once the best page so far rates at least F "
             "and the K pages after it rate no higher\". Each cell: answers kept / answers the full crawl found, "
             f"mean pages fetched (budget {qc.BUDGET}). A stop keeps the {qc.CONCURRENCY - 1} pages in flight. "
             f"Bold: the rule `--stop-when-answered` ships, K={SHIPPED[0]}, F={SHIPPED[1]}."]
    for s in sets:
        g = grids[s]
        lines += ["", f"## {s}", "", "| K | " + " | ".join(f"F={f}" for f in FLOORS) + " |",
                  "|---:|" + "---:|" * len(FLOORS)]
        for k in KS:
            lines.append(f"| {k} | " + " | ".join(
                f"{'**' if (k, f) == SHIPPED else ''}{g[k, f]['kept']}/{g[k, f]['found']} {g[k, f]['pages']:.1f}"
                f"{'**' if (k, f) == SHIPPED else ''}" for f in FLOORS) + " |")
        shipped = g[SHIPPED]
        lines += ["", f"Shipped rule on {s}: {shipped['kept']}/{shipped['found']} answers kept, "
                  f"{shipped['pages']:.1f} pages on average, answer in the top {qc.TOP} for {shipped['top']}."]
    (qc.ROOT / "results" / label / "query-crawl-stop-replay.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main(sys.argv[1])
