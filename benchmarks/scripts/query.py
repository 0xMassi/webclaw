import json, os, re, statistics, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
QUESTIONS = ROOT / "questions.jsonl"
ARM_VENV = {"webclaw-llm": "base", "webclaw-query": "base", "scrapling": "scrapling",
            "crawl4ai-raw": "crawl4ai", "crawl4ai-bm25": "crawl4ai"}
CONTROLS = {"webclaw-query": "webclaw-llm", "crawl4ai-bm25": "crawl4ai-raw"}


def hits(text, answers):
    low = text.lower()
    n = 0
    for a in answers:
        al = a.lower()
        if " " in a or not a.isascii() or not a.isalpha():
            n += al in low
        else:
            n += bool(re.search(r"\b" + re.escape(al) + r"\b", low))
    return n


def build_cases():
    pages = {c["id"]: c for c in map(json.loads, open(WORK / "cases-html.jsonl"))}
    cases, counts = [], {}
    for q in map(json.loads, open(QUESTIONS)):
        page = pages.get(q["id"])
        if page is None:
            sys.exit(f"{q['id']} has no frozen page; run ./run.sh once to build .work/corpus")
        counts[q["id"]] = counts.get(q["id"], 0) + 1
        cases.append({**page, "id": f"{q['id']}-q{counts[q['id']]}", "question": q["question"], "facts": q["answers"]})
    path = WORK / "query-cases.jsonl"
    path.write_text("".join(json.dumps(c) + "\n" for c in cases))
    return path, cases


def main(label):
    import tiktoken
    enc = tiktoken.get_encoding("cl100k_base")
    cases_path, cases = build_cases()
    res = ROOT / "results" / label
    out = WORK / "query-out" / label
    res.mkdir(parents=True, exist_ok=True)
    rows_path = res / "query-rows.jsonl"
    rows_path.unlink(missing_ok=True)
    for arm, venv in ARM_VENV.items():
        py = str(WORK / f"venv-{venv}" / "bin/python")
        subprocess.run([py, str(ROOT / "scripts/arms.py"), arm, "html", str(cases_path), str(rows_path),
                        str(out / arm), "1"], check=True)
    by_case = {c["id"]: c for c in cases}
    rows = list(map(json.loads, open(rows_path)))
    texts = {}
    for row in rows:
        text = (out / row["arm"] / f"{row['id']}-0.md").read_text()
        texts[(row["arm"], row["id"])] = text
        c = by_case[row["id"]]
        row.update(hits=hits(text, c["facts"]), nfacts=len(c["facts"]), tokens=len(enc.encode(text, disallowed_special=())),
                   english=c["category"] != "nonenglish")
    for filtered, full in CONTROLS.items():
        for row in [r for r in rows if r["arm"] == filtered]:
            cut = enc.decode(enc.encode(texts[(full, row["id"])], disallowed_special=())[:row["tokens"]])
            c = by_case[row["id"]]
            rows.append({**row, "arm": f"{full} cut to {filtered} size", "hits": hits(cut, c["facts"]), "seconds": 0})
    rows_path.write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# Query-focused extraction {label}", "",
             f"{len(cases)} questions over {len({c['html_path'] for c in cases})} frozen pages (`questions.jsonl`). "
             "Each question was written apart from its answers: no answer string appears in its question. Answer "
             "recall counts answers found in the output, with failed runs as misses; `ok` counts runs that returned "
             "text without an error. Tokens: cl100k, median over ok runs. The `cut to` rows truncate the "
             "tool's full output to the token count its own query filter returned for the same question, so a "
             "filter has to beat keeping the top of the page.", ""]
    for scope, keep in (("all questions", lambda r: True), ("English pages", lambda r: r["english"]),
                        ("non-English pages", lambda r: not r["english"])):
        lines += [f"## {scope}", "", "| arm | questions | ok | answer recall % | median tokens | p50 s |",
                  "|---|---:|---:|---:|---:|---:|"]
        arms = list(ARM_VENV) + [f"{full} cut to {filtered} size" for filtered, full in CONTROLS.items()]
        for arm in arms:
            group = [r for r in rows if r["arm"] == arm and keep(r)]
            if not group:
                continue
            ok = [r for r in group if r["ok"]]
            lines.append(f"| {arm} | {len(group)} | {len(ok)} | "
                         f"{100 * sum(r['hits'] for r in group) / sum(r['nfacts'] for r in group):.1f} | "
                         f"{int(statistics.median(r['tokens'] for r in ok)) if ok else 0} | "
                         f"{statistics.median(r['seconds'] for r in group):.2f} |")
        lines.append("")
    (res / "query-summary.md").write_text("\n".join(lines))
    print("\n".join(lines))


main(sys.argv[1])
