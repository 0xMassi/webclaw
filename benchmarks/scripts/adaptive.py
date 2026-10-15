import json, os, subprocess, sys, time
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
CORPUS = WORK / "adaptive-corpus"
ARMS = [("selector", "base"), ("text-fallback", "base"), ("scrapling-adaptive", "scrapling"),
        ("scrapling-relocate", "scrapling"), ("crawl4ai", "crawl4ai")]
ARM_NOTES = {
    "selector": "webclaw `--select` with the old selector, nothing else",
    "text-fallback": "old selector; when it matches nothing, webclaw `--find-text` for the saved text, exact",
    "scrapling-adaptive": "Scrapling `auto_save=True` on the old page, `adaptive=True` on the new one",
    "scrapling-relocate": "Scrapling `relocate()` with the saved element, selector ignored",
}


def webclaw(*args):
    r = subprocess.run([os.environ["WEBCLAW_BIN"], *args], capture_output=True, text=True)
    if r.returncode:
        sys.exit(f"webclaw {' '.join(args)} failed: {r.stderr}")
    return r.stdout


def freeze(cases):
    CORPUS.mkdir(parents=True, exist_ok=True)
    for case in cases:
        for version in ("old", "new"):
            page = CORPUS / f"{case['name']}.{version}.html"
            if not page.exists():
                page.write_text(webclaw(f"https://web.archive.org/web/{case[version]}id_/{case['url']}", "--raw-html"))
                time.sleep(5)


class Key(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.root, self.text, self.skip = None, [], 0

    def handle_starttag(self, tag, attrs):
        if self.root is None:
            self.root = (tag, tuple(sorted((k, " ".join((v or "").split())) for k, v in attrs)))
        if tag in ("script", "style"):
            self.skip += 1

    def handle_endtag(self, tag):
        if tag in ("script", "style") and self.skip:
            self.skip -= 1

    def handle_data(self, data):
        if not self.skip:
            self.text.append(data)


def key(html):
    if html is None:
        return None
    k = Key()
    k.feed(html)
    return k.root, "".join("".join(k.text).split())


def first(name, version, css, take):
    got = json.loads(webclaw("--file", str(CORPUS / f"{name}.{version}.html"), "--select", f"{css}::{take}"))
    if not got:
        sys.exit(f"{name}: `{css}` picks nothing on the {version} page")
    return got[0]


def outcome(got, truth):
    if got is None:
        return "none"
    return "right" if key(got) in truth else "wrong"


def main(label):
    cases = json.loads((ROOT / "adaptive-cases.json").read_text())
    freeze(cases)
    flat = []
    for case in cases:
        for target in case["targets"]:
            target["text"] = first(case["name"], "old", target["old"], "text")
            target["truth"] = [first(case["name"], "new", css, "html") for css in [target["new"], *target.get("also", [])]]
            flat.append((case, target))
    out_dir = WORK / "adaptive-out" / label
    out_dir.mkdir(parents=True, exist_ok=True)
    cases_file = out_dir / "cases.json"
    cases_file.write_text(json.dumps(cases))
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    truths = [[key(h) for h in t["truth"]] for _, t in flat]
    per_arm, notes = {}, {}
    for arm, venv in ARMS:
        out = out_dir / f"{arm}.json"
        py = str(WORK / f"venv-{venv}" / "bin/python")
        subprocess.run([py, str(ROOT / "scripts/adaptive_arms.py"), arm, str(CORPUS), str(cases_file), str(out)],
                       check=True)
        got = json.loads(out.read_text())
        if not got["supported"]:
            notes[arm] = got["note"]
            continue
        if arm == "scrapling-relocate":
            scores = [r["score"] for r in got["targets"]]
            raw = [outcome(r["html"], k) for r, k in zip(got["targets"], truths)]
            for r in got["targets"]:
                r["html"] = r["html"] if r["score"] >= 40 else None
        per_arm[arm] = {"outcomes": [outcome(r["html"], k) for r, k in zip(got["targets"], truths)],
                        "seconds": got["seconds"]}
    drifted = [o != "right" for o in per_arm["selector"]["outcomes"]]
    rows = []
    for arm, data in per_arm.items():
        o = data["outcomes"]
        row = {"arm": arm, "targets": len(o), "right": o.count("right"), "wrong": o.count("wrong"),
               "none": o.count("none"),
               "drifted_right": sum(x == "right" for x, d in zip(o, drifted) if d),
               "drifted_wrong": sum(x == "wrong" for x, d in zip(o, drifted) if d),
               "stable_right": sum(x == "right" for x, d in zip(o, drifted) if not d),
               "seconds": data["seconds"]}
        rows.append(row)
        print(json.dumps(row), flush=True)
    per_target = [{"page": c["name"], "field": t["field"], "old": t["old"], "new": t["new"],
                   **{arm: data["outcomes"][i] for arm, data in per_arm.items()},
                   "relocate_score": scores[i], "relocate_best": raw[i]}
                  for i, (c, t) in enumerate(flat)]
    (res / "adaptive-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows + per_target))
    sel = per_arm["selector"]["outcomes"]
    pages_drifted = len({c["name"] for (c, _), d in zip(flat, drifted) if d})
    n, nd = len(flat), sum(drifted)
    lines = [f"# Adaptive selector benchmark {label}", "",
             f"{len(cases)} pages, each frozen twice from the Wayback Machine years apart into `.work/adaptive-corpus/`, "
             f"{n} target fields. Each field has a CSS selector written on the old page and a hand-checked selector "
             "for the same field on the new page (`adaptive-cases.json`). An answer is right when its first element has "
             "the tag, attributes and text of the field's element on the new page.", "",
             f"Baseline drift: the old selector picks the right element on the new page for {sel.count('right')}/{n} "
             f"fields. It picks nothing for {sel.count('none')} and a different element for {sel.count('wrong')} "
             f"(silent drift). {pages_drifted}/{len(cases)} pages have at least one drifted field.", "",
             "| arm | right | wrong | none | right on drifted | wrong on drifted | right on stable | seconds |",
             "|---|---:|---:|---:|---:|---:|---:|---:|"]
    for r in rows:
        lines.append(f"| {r['arm']} | {r['right']}/{n} | {r['wrong']} | {r['none']} | {r['drifted_right']}/{nd} | "
                     f"{r['drifted_wrong']} | {r['stable_right']}/{n - nd} | {r['seconds']:.2f} |")
    for arm, note in notes.items():
        lines.append(f"| {arm} | {note} |" + " - |" * 6)
    lines += ["", "Scrapling's relocation keeps its best-scoring element when the score is at least `percentage` "
              "(default 40). The same `adaptive=True` run with a higher threshold, using the scores of the "
              "`scrapling-relocate` arm for the fields whose old selector picks nothing:", "",
              "| threshold | right | wrong | none | right on drifted | wrong on drifted |", "|---:|---:|---:|---:|---:|---:|"]
    for th in (40, 50, 60, 70, 80, 90):
        o = [r if s_ == "none" and sc >= th else ("none" if s_ == "none" else s_)
             for s_, r, sc in zip(sel, raw, scores)]
        lines.append(f"| {th} | {o.count('right')}/{n} | {o.count('wrong')} | {o.count('none')} | "
                     f"{sum(x == 'right' for x, d in zip(o, drifted) if d)}/{nd} | "
                     f"{sum(x == 'wrong' for x, d in zip(o, drifted) if d)} |")
    lines += ["", "Arms:", ""] + [f"- `{a}`: {t}" for a, t in ARM_NOTES.items() if a in per_arm]
    lines += ["", "## Per field", "", "`relocate score` is Scrapling's score for its best element and whether that "
              "element is right.", "", "| page | field | " + " | ".join(per_arm) + " | relocate score |",
              "|---|---|" + "---|" * len(per_arm) + "---:|"]
    lines += [f"| {p['page']} | {p['field']} | " + " | ".join(p[a] for a in per_arm)
              + f" | {p['relocate_score']:.1f} {p['relocate_best']} |" for p in per_target]
    (res / "adaptive-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
