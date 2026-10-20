import json, os, subprocess, sys, tempfile, time
from pathlib import Path

ARM, CORPUS, CASES, OUT = sys.argv[1], Path(sys.argv[2]), json.loads(Path(sys.argv[3]).read_text()), Path(sys.argv[4])


def page(case, version):
    return str(CORPUS / f"{case['name']}.{version}.html")


def webclaw(*args):
    r = subprocess.run([os.environ["WEBCLAW_BIN"], *args], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def first_html(case, version, css):
    out = webclaw("--file", page(case, version), "--select", f"{css}::html")
    values = json.loads(out) if out else []
    return values[0] if values else None


def webclaw_selector(case, target):
    return {"html": first_html(case, "new", target["old"])}


def webclaw_text_fallback(case, target):
    html = first_html(case, "new", target["old"])
    if html is not None:
        return {"html": html}
    out = webclaw("--file", page(case, "new"), "--find-text", target["text"], "--exact")
    found = json.loads(out) if out else []
    return {"html": first_html(case, "new", found[0]["selector"]) if found else None, "fallback": True}


def scrapling_arm(relocate_always):
    from scrapling import Selector
    from scrapling.core.utils import _StorageTools
    from lxml.html import tostring

    def outer(el):
        return tostring(el, encoding="unicode", method="html", with_tail=False)

    db = tempfile.mkdtemp()

    def run(case, target):
        old = Path(page(case, "old")).read_bytes()
        new = Path(page(case, "new")).read_bytes()
        if relocate_always:
            found = Selector(old).css(target["old"])
            saved = _StorageTools.element_to_dict(found[0]._root)
            page_ = Selector(new)
            got = page_.relocate(saved, percentage=0)
            score = page_._Selector__calculate_similarity_score(saved, got[0])
            return {"html": outer(got[0]), "ties": len(got), "score": score}
        args = {"storage_args": {"storage_file": f"{db}/{case['name']}.db", "url": case["url"]}}
        Selector(old, url=case["url"], adaptive=True, **args).css(target["old"], auto_save=True)
        got = Selector(new, url=case["url"], adaptive=True, **args).css(target["old"], adaptive=True)
        return {"html": got[0].html_content if got else None}

    return run


ARMS = {
    "selector": webclaw_selector,
    "text-fallback": webclaw_text_fallback,
    "scrapling-adaptive": lambda: scrapling_arm(False),
    "scrapling-relocate": lambda: scrapling_arm(True),
}
if ARM == "crawl4ai":
    OUT.write_text(json.dumps({"supported": False, "note": "no equivalent (no saved-element relocation)"}))
    sys.exit()
fn = ARMS[ARM]
if ARM.startswith("scrapling"):
    fn = fn()
results = []
start = time.perf_counter()
for case in CASES:
    for target in case["targets"]:
        t = time.perf_counter()
        r = fn(case, target)
        r["seconds"] = time.perf_counter() - t
        results.append(r)
OUT.write_text(json.dumps({"supported": True, "targets": results, "seconds": time.perf_counter() - start}))
