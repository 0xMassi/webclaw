"""Markdown with numbered link citations, driven by ../citations.sh. Runs under .work/venv-base.

  citations.py <label>    results/<label>/citations-summary.md and citations-rows.jsonl

html mode: every arm reads the pages frozen by run.py prepare, without a page URL, so links keep
the form the page wrote them in, and both outputs of an arm come from the same input. Links are
parsed from each tool's plain markdown with one shared regex, then looked up in that tool's
citation output: the URL must be in the references, and the body must carry the link's text
followed by that URL's number. url mode: every arm fetches the live page, so links are absolute;
webclaw fetches twice (plain, then --citations), so only tokens and facts are scored there.
"""
import json, os, re, statistics, subprocess, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run import hits

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CASES = WORK / "cases-html.jsonl"
ARMS = {"webclaw": "venv-base", "crawl4ai": "venv-crawl4ai"}
ROUNDS = {"html": 3, "url": 1}
LINK = re.compile(r'!?\[((?:[^\[\]]|\[(?:[^\[\]]|\[[^\]]*\])*\])*)\]\(((?:[^()\s]|\([^()]*\))*)(?:\s+"([^"]*)")?\)')
FENCE = re.compile(r"^(```|~~~).*?^\1[^\n]*$", re.M | re.S)
INLINE_CODE = re.compile(r"`[^`\n]*`")
STYLE = {"webclaw": (re.compile(r"^\[\^(\d+)\]: (\S+)(.*)$", re.M), re.compile(r"\[\^(\d+)\]"), "{}[^{}]", "## Links\n"),
         "crawl4ai": (re.compile(r"^⟨(\d+)⟩ (\S+)(.*)$", re.M), re.compile(r"⟨(\d+)⟩"), "{}⟨{}⟩", "## References\n")}


def split_refs(arm, cited):
    head, sep, refs = cited.rpartition(STYLE[arm][3])
    if not sep:
        return cited, []
    entries = []
    for n, url, rest in STYLE[arm][0].findall(refs.split("\n## Structured Data")[0]):
        if rest.startswith(" ") and url.endswith(":"):
            url = url[:-1]
        entries.append((int(n), url))
    return head, entries


def well_numbered(arm, body, entries):
    numbers = [n for n, _ in entries]
    first_seen = list(dict.fromkeys(int(n) for n in STYLE[arm][1].findall(body)))
    return (numbers == list(range(1, len(entries) + 1)) and len({u for _, u in entries}) == len(entries)
            and first_seen == sorted(first_seen))


def links(plain):
    text = INLINE_CODE.sub("", FENCE.sub("", plain))
    return [(m.group(0).startswith("!"), m.group(1), m.group(2)) for m in LINK.finditer(text)]


def score(arm, plain, cited):
    body, entries = split_refs(arm, cited)
    _, mark, cite, _ = STYLE[arm]
    numbers = dict((u, n) for n, u in reversed(entries))
    found = links(plain)
    page_links = [(t, u) for image, t, u in found if not image]
    images = [u for image, _, u in found if image]
    dropped = [u for _, u in page_links if arm == "webclaw" and (u.startswith("#") or u.lower().startswith("javascript:"))]
    citable = [(t, u) for t, u in page_links if u not in dropped]
    url_ok = [u for _, u in citable if u in numbers]
    checkable = [(t, u) for t, u in citable if "](" not in t and t.strip()]
    text_ok = [1 for t, u in checkable if u in numbers and cite.format(t, numbers[u]) in body]
    plain_fences = FENCE.findall(plain)
    cited_fences = [m.group(0) for m in FENCE.finditer(body)]
    code_marks = sum(len(mark.findall(f)) for f in cited_fences)
    return {"links": len(citable), "links_url_ok": len(url_ok), "links_checked": len(checkable),
            "links_text_ok": len(text_ok), "anchors_dropped": len(dropped), "images": len(images),
            "images_cited": sum(u in numbers for u in images), "refs": len(entries),
            "well_numbered": well_numbered(arm, body, entries), "code_marks": code_marks,
            "fences": len(plain_fences)}


def angle_marks(cited):
    head, sep, refs = cited.rpartition("\n\n" + STYLE["webclaw"][3])
    if not sep:
        return cited
    head = re.sub(r"!\[([^\]]*)\]\[\^(\d+)\]", r"![\1⟨\2⟩]", head)
    head = re.sub(r"\[\^(\d+)\]", r"⟨\1⟩", head)
    return head + sep + re.sub(r"^\[\^(\d+)\]: ", r"⟨\1⟩ ", refs, flags=re.M)


def pct(a, b):
    return f"{100 * a / b:.1f}" if b else "-"


def main(label):
    import tiktoken
    enc = tiktoken.get_encoding("cl100k_base")
    tokens = lambda s: len(enc.encode(s, disallowed_special=()))
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    cases = {c["id"]: c for c in map(json.loads, open(CASES))}
    rows = []
    for mode in ROUNDS:
        for arm, venv in ARMS.items():
            out = WORK / "citations-out" / f"{arm}-{mode}.jsonl"
            out.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run([str(WORK / venv / "bin/python"), str(ROOT / "scripts/citations_arms.py"), arm, mode,
                            str(CASES), str(out), str(ROUNDS[mode])], check=True)
            rows += [scored(arm, mode, row, cases[row["id"]], tokens) for row in map(json.loads, open(out))]
    (res / "citations-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    version = subprocess.run([os.environ["WEBCLAW_BIN"], "--version"], capture_output=True, text=True).stdout.strip()
    lines = [f"# Citations benchmark {label}", "", f"webclaw: {version}; other tool versions: [versions.txt](versions.txt)", "",
             f"{len(cases)} pages frozen by `run.sh`. html mode reads them from disk without a page URL, 3 rounds; "
             "url mode fetches each live page once per output. Tokens: cl100k. Seconds: median per page. "
             "Scrapling has no citation output (its markdown comes from markdownify with links inline), so it has no row.", "",
             "Saved % is over the total of all pages; the median page saving and the top 5 pages' share of the total saving show how even it is.", "",
             "| mode | arm | pages | median tokens plain | median tokens cited | total tokens plain | total tokens cited | saved % | median page saved % | pages longer | top 5 pages' share of saving % | median body tokens | fact recall plain % | fact recall cited % |",
             "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for mode in ROUNDS:
        for arm in ARMS:
            ok = [r for r in rows if r["arm"] == arm and r["mode"] == mode and not r["error"]]
            if not ok:
                continue
            s = lambda k: sum(r[k] for r in ok)
            med = lambda k: int(statistics.median(r[k] for r in ok))
            saved = sorted((r["tokens_plain"] - r["tokens_cited"] for r in ok), reverse=True)
            page_pct = statistics.median(100 * (r["tokens_plain"] - r["tokens_cited"]) / r["tokens_plain"] for r in ok)
            longer = sum(r["tokens_cited"] > r["tokens_plain"] for r in ok)
            lines.append(f"| {mode} | {arm} | {len(ok)} | {med('tokens_plain'):,} | {med('tokens_cited'):,} | {s('tokens_plain'):,} | "
                         f"{s('tokens_cited'):,} | {pct(s('tokens_plain') - s('tokens_cited'), s('tokens_plain'))} | {page_pct:.1f} | {longer} | "
                         f"{pct(sum(saved[:5]), sum(saved)) if sum(saved) > 0 else '-'} | {med('tokens_body'):,} | "
                         f"{pct(s('hits_plain'), s('facts'))} | {pct(s('hits_cited'), s('facts'))} |")
    lines += ["", "Mark style: the same webclaw citation output, its footnote marks `[^1]` swapped for the other tool's `⟨1⟩` "
              "(body and list, same heading), counted again:", "",
              "| mode | pages | total tokens plain | footnote marks | saved % | angle marks | saved % |", "|---|---:|---:|---:|---:|---:|---:|"]
    for mode in ROUNDS:
        ok = [r for r in rows if r["arm"] == "webclaw" and r["mode"] == mode and not r["error"]]
        if ok:
            p, f, a = (sum(r[k] for r in ok) for k in ("tokens_plain", "tokens_cited", "tokens_angle"))
            lines.append(f"| {mode} | {len(ok)} | {p:,} | {f:,} | {pct(p - f, p)} | {a:,} | {pct(p - a, p)} |")
    lines += ["", "html mode:", "", "| arm | links | URL in references % | text then right number % | same-page anchors dropped | images | images cited % | "
              "numbered 1..N, one per URL, by first appearance | citation marks inside code blocks | p50 s plain | p50 s cited | p50 ms citation step |",
              "|---|---:|---:|---:|---:|---:|---:|---|---:|---:|---:|---:|"]
    for arm in ARMS:
        ok = [r for r in rows if r["arm"] == arm and r["mode"] == "html" and not r["error"]]
        if not ok:
            continue
        s = lambda k: sum(r[k] for r in ok)
        med = lambda k: statistics.median(r[k] for r in ok)
        dedup = sum(r["well_numbered"] for r in ok)
        step = f"{1000 * med('seconds_step'):.2f}" if ok[0]["seconds_step"] is not None else "-"
        lines.append(f"| {arm} | {s('links'):,} | {pct(s('links_url_ok'), s('links'))} | {pct(s('links_text_ok'), s('links_checked'))} | "
                     f"{s('anchors_dropped'):,} | {s('images'):,} | {pct(s('images_cited'), s('images'))} | {dedup}/{len(ok)} pages | "
                     f"{s('code_marks')} in {s('fences')} blocks | {med('seconds_plain'):.3f} | {med('seconds_cited'):.3f} | {step} |")
    failed = [r for r in rows if r["error"]]
    if failed:
        lines += ["", "Failed pages:", ""] + [f"- {r['mode']} {r['arm']} {r['id']}: {r['error']}" for r in failed]
    (res / "citations-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


def scored(arm, mode, row, case, tokens):
    if row["error"]:
        return {"arm": arm, "mode": mode, "id": row["id"], "error": row["error"]}
    body, _ = split_refs(arm, row["cited"])
    return {"arm": arm, "mode": mode, "id": row["id"], "error": None,
            "tokens_plain": tokens(row["plain"]), "tokens_cited": tokens(row["cited"]),
            "tokens_body": tokens(body), "facts": len(case["facts"]),
            "tokens_angle": tokens(angle_marks(row["cited"])) if arm == "webclaw" else None,
            "hits_plain": hits(row["plain"], case["facts"]), "hits_cited": hits(row["cited"], case["facts"]),
            "seconds_plain": row["seconds_plain"], "seconds_cited": row["seconds_cited"],
            "seconds_step": row.get("seconds_step"), **score(arm, row["plain"], row["cited"])}


if __name__ == "__main__":
    main(sys.argv[1])
