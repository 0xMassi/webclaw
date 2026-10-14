"""Chunking for retrieval, driven by ../chunking.sh. Runs under .work/venv-base (needs tiktoken).

Every arm chunks the pages frozen by run.sh at the same target sizes. Scores: golden case facts that sit
whole inside one chunk, BM25 top-1/top-3 answer hits for questions.jsonl with one scorer shared by every
arm, chunk sizes in cl100k tokens against the target, and seconds per page from the frozen HTML.
"""
import json, math, os, re, shutil, statistics, subprocess, sys, time
from collections import Counter, defaultdict
from pathlib import Path

from run import hits, pct

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
WEBCLAW = os.environ["WEBCLAW_BIN"]
SIZES = [256, 512]
WORD = re.compile(r"\w+")


def is_cjk(c):
    return "぀" <= c <= "ヿ" or "㐀" <= c <= "䶿" or "一" <= c <= "鿿" or "가" <= c <= "힯"


def terms(text):
    out = []
    for w in WORD.findall(text.lower()):
        out += [w[i:i + 2] for i in range(len(w) - 1)] or [w] if any(map(is_cjk, w)) else [w]
    return out


def bm25_rank(docs, query, k1=1.2, b=0.75):
    wanted = set(terms(query))
    toks = [terms(d) for d in docs]
    avg = statistics.mean(map(len, toks)) or 1
    df = Counter(t for d in toks for t in set(d) if t in wanted)
    scores = []
    for d in toks:
        tf = Counter(t for t in d if t in wanted)
        scores.append(sum(math.log((len(docs) - df[t] + .5) / (df[t] + .5) + 1) * f * (k1 + 1)
                          / (f + k1 * (1 - b + b * len(d) / avg)) for t, f in tf.items()))
    return sorted(range(len(docs)), key=lambda i: -scores[i])


def webclaw(page, *args):
    t0 = time.perf_counter()
    r = subprocess.run([WEBCLAW, "--file", page["html_path"], "--render", "never", *args],
                       capture_output=True, text=True, timeout=60)
    return r.stdout, time.perf_counter() - t0


def tool_markdown(arm, venv, pages, out):
    rows = out / f"{arm}-rows.jsonl"
    subprocess.run([str(WORK / f"venv-{venv}/bin/python"), str(ROOT / "scripts/arms.py"), arm, "html",
                    str(WORK / "cases-html.jsonl"), str(rows), str(out / arm), "1"], check=True)
    secs = {r["id"]: r["seconds"] for r in map(json.loads, open(rows))}
    folder = out / "md" / arm
    folder.mkdir(parents=True)
    for p in pages:
        (folder / f"{p['id']}.md").write_text((out / arm / f"{p['id']}-0.md").read_text())
    return folder, secs


def main(label):
    import tiktoken
    enc = tiktoken.get_encoding("cl100k_base")
    res = ROOT / "results" / label
    out = WORK / "chunking-out" / label
    shutil.rmtree(out, ignore_errors=True)
    res.mkdir(parents=True, exist_ok=True)
    pages = [json.loads(l) for l in open(WORK / "cases-html.jsonl")]
    for p in pages:
        p["html_path"] = str(WORK / p["html_path"])
    questions = defaultdict(list)
    for q in map(json.loads, open(ROOT / "questions.jsonl")):
        questions[q["id"]].append(q)

    md, md_secs = {}, {}
    wc_dir = out / "md" / "webclaw"
    wc_dir.mkdir(parents=True)
    md_secs["webclaw"] = {}
    for p in pages:
        text, secs = webclaw(p, "-f", "json")
        (wc_dir / f"{p['id']}.md").write_text(json.loads(text)["content"]["markdown"] if text.strip() else "")
        md_secs["webclaw"][p["id"]] = secs
    folders = {"webclaw": wc_dir}
    for tool, arm, venv in (("crawl4ai", "crawl4ai-raw", "crawl4ai"), ("scrapling", "scrapling", "scrapling")):
        folders[tool], md_secs[tool] = tool_markdown(arm, venv, pages, out)
    for tool, folder in folders.items():
        md[tool] = {p["id"]: (folder / f"{p['id']}.md").read_text() for p in pages}

    runs = []
    for size in SIZES:
        for overlap in (0, size // 10):
            arm = "webclaw --chunk" + (f" --chunk-overlap {overlap}" if overlap else "")
            for p in pages:
                text, secs = webclaw(p, "--chunk", str(size), "--chunk-overlap", str(overlap))
                chunks = [json.loads(l) for l in text.splitlines() if l.strip()]
                runs.append({"arm": arm, "source": "webclaw", "size": size, "id": p["id"], "seconds": secs,
                             "chunks": [c["text"] for c in chunks], "estimates": [c["tokens"] for c in chunks],
                             "paths": [" ".join(c["heading_path"]) for c in chunks]})
    b_rows = out / "crawl4ai-chunks.jsonl"
    subprocess.run([str(WORK / "venv-crawl4ai/bin/python"), str(ROOT / "scripts/chunking_arms.py"), str(b_rows),
                    ",".join(map(str, SIZES)), f"crawl4ai={folders['crawl4ai']}", f"webclaw={folders['webclaw']}"],
                   check=True)
    for r in map(json.loads, open(b_rows)):
        own = r["source"] == "crawl4ai"
        runs.append({"arm": f"crawl4ai {r['strategy']}" + ("" if own else " on webclaw markdown"),
                     "source": r["source"], "size": r["size"], "id": r["id"],
                     "seconds": md_secs[r["source"]][r["id"]] + r["seconds"], "chunks": r["chunks"]})
    for size in SIZES:
        for p in pages:
            text = md["scrapling"][p["id"]]
            t0 = time.perf_counter()
            chunks = [c for c in (text[i:i + size * 4] for i in range(0, len(text), size * 4)) if c.strip()]
            runs.append({"arm": "scrapling (no chunker): fixed 4-chars-per-token split", "source": "scrapling",
                         "size": size, "id": p["id"], "chunks": chunks,
                         "seconds": md_secs["scrapling"][p["id"]] + time.perf_counter() - t0})
    for r in [r for r in runs if "paths" in r and r["arm"] == "webclaw --chunk"]:
        runs.append({**r, "arm": "webclaw --chunk, heading path scored with the text",
                     "chunks": [f"{h}\n{t}" for h, t in zip(r["paths"], r["chunks"])]})

    rows = []
    for r in runs:
        p = next(x for x in pages if x["id"] == r["id"])
        source = md[r["source"]][r["id"]]
        present = [f for f in p["facts"] if hits(source, [f])]
        whole = [f for f in present if any(hits(c, [f]) for c in r["chunks"])]
        tokens = [len(enc.encode(c, disallowed_special=())) for c in r["chunks"]]
        q_rows = []
        for q in questions.get(r["id"], []):
            order = bm25_rank(r["chunks"], q["question"]) if r["chunks"] else []
            found = [bool(hits(r["chunks"][i], q["answers"])) for i in order]
            q_rows.append({"top1": any(found[:1]), "top3": any(found[:3]), "any": any(found),
                           "top3_tokens": sum(tokens[i] for i in order[:3])})
        rows.append({"arm": r["arm"], "size": r["size"], "id": r["id"], "seconds": r["seconds"],
                     "facts_present": len(present), "facts_whole": len(whole), "tokens": tokens,
                     "estimates": r.get("estimates"), "questions": q_rows})
    (res / "chunking-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))

    arms = list(dict.fromkeys(r["arm"] for r in rows))
    lines = [f"# Chunking for retrieval {label}", "",
             f"{len(pages)} pages frozen by `run.sh`, {sum(map(len, questions.values()))} questions from "
             "`questions.jsonl`. Facts: golden case facts found in the markdown the arm chunked; `whole` counts "
             "those that sit inside one chunk. Retrieval: one BM25 scorer (k1 1.2, b 0.75, lowercase words, CJK "
             "character pairs, no stemming) ranks each page's chunks against the question; a hit is a chunk "
             "holding an answer. `reachable` is any chunk holding it. Sizes are cl100k tokens. `±25%` is the share "
             "of chunks within a quarter of the target; `over` the share above 125%. Seconds per page run from the "
             "frozen HTML: markdown plus chunking. Crawl4AI's window sizes are words: the target over its own "
             "1.3 tokens-per-word rate. `regex + merge` is its LLM extraction path (paragraph split, then "
             "`merge_chunks` to the token target with 10% overlap). Scrapling has no chunker; its row cuts its "
             "markdown every 4 characters per target token, the split a user would write.", ""]
    for size in SIZES:
        lines += [f"## target {size} tokens", "",
                  "| arm | chunks | median tokens | p95 tokens | ±25% | over | facts whole | top-1 | top-3 | "
                  "reachable | top-3 tokens | p50 s/page |",
                  "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
        for arm in arms:
            group = [r for r in rows if r["arm"] == arm and r["size"] == size]
            if not group:
                continue
            toks = [t for r in group for t in r["tokens"]]
            qs = [q for r in group for q in r["questions"]]
            present = sum(r["facts_present"] for r in group)
            share = lambda f: 100 * sum(map(f, toks)) / len(toks) if toks else 0
            lines.append(
                f"| {arm} | {len(toks)} | {int(statistics.median(toks)) if toks else 0} | {pct(toks, .95)} | "
                f"{share(lambda t: abs(t - size) <= size / 4):.0f}% | {share(lambda t: t > size * 1.25):.0f}% | "
                f"{sum(r['facts_whole'] for r in group)}/{present} | "
                f"{sum(q['top1'] for q in qs)}/{len(qs)} | {sum(q['top3'] for q in qs)}/{len(qs)} | "
                f"{sum(q['any'] for q in qs)}/{len(qs)} | {int(statistics.median(q['top3_tokens'] for q in qs))} | "
                f"{pct([r['seconds'] for r in group], .5):.3f} |")
        lines.append("")
    ratios = [t / max(e, 1) for r in rows if r["arm"] == "webclaw --chunk" and r["estimates"]
              for t, e in zip(r["tokens"], r["estimates"])]
    lines += [f"webclaw's `tokens` estimate against cl100k over {len(ratios)} chunks: cl100k / estimate p5 "
              f"{pct(ratios, .05):.2f}, p50 {pct(ratios, .5):.2f}, p95 {pct(ratios, .95):.2f}.", ""]
    split = [(r["arm"], r["size"], r["id"]) for r in rows if r["facts_whole"] < r["facts_present"]]
    if split:
        lines += ["Pages with a golden fact split across chunks: " +
                  "; ".join(f"{a} @{s}: {i}" for a, s, i in split), ""]
    (res / "chunking-summary.md").write_text("\n".join(lines))
    print("\n".join(lines))


main(sys.argv[1])
