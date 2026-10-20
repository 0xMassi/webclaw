import json, os, subprocess, sys, time
from pathlib import Path

ARM, CORPUS, CASES, OUT = sys.argv[1], Path(sys.argv[2]), json.loads(Path(sys.argv[3]).read_text()), Path(sys.argv[4])


def webclaw(case):
    bin_ = os.environ["WEBCLAW_BIN"]
    page = str(CORPUS / f"{case['name']}.html")

    def run(*args):
        r = subprocess.run([bin_, "--file", page, *args], capture_output=True, text=True)
        return json.loads(r.stdout) if r.returncode == 0 else None

    found = run("--find-text", case["example"], "--exact")
    start = time.perf_counter()
    similar = run("--find-text", case["example"], "--exact", "--similar")
    seconds = time.perf_counter() - start
    from_css = run("--select", case["gt"], "--similar")
    return {"text_first": found[0]["text"] if found else None,
            "from_text": [m["text"] for m in similar["matches"]] if similar else None,
            "from_css": [m["text"] for m in from_css["matches"]] if from_css else None,
            "selector": similar and similar["selector"],
            "reuse": run("--select", similar["selector"]) if similar else None,
            "seconds": seconds}


def scrapling(case):
    from scrapling import Selector

    def text(el):
        return " ".join(el.get_all_text(separator="").split())

    def similar(el):
        return [text(e) for e in [el, *el.find_similar()]]

    html = (CORPUS / f"{case['name']}.html").read_text()
    found = Selector(html, adaptive=False).find_by_text(case["example"], first_match=False)
    start = time.perf_counter()
    example = Selector(html, adaptive=False).find_by_text(case["example"])
    from_text = similar(example) if example else None
    seconds = time.perf_counter() - start
    first = Selector(html, adaptive=False).css(case["gt"])
    return {"text_first": text(found[0]) if found else None, "from_text": from_text,
            "from_css": similar(first[0]) if first else None, "seconds": seconds}


if ARM == "crawl4ai":
    OUT.write_text(json.dumps({"supported": False,
                               "note": "no equivalent (its schema generator needs an LLM call)"}))
    sys.exit()
if ARM == "webclaw":
    for _ in range(3):
        subprocess.run([os.environ["WEBCLAW_BIN"], "--version"], capture_output=True, check=True)
pages = [{"webclaw": webclaw, "scrapling": scrapling}[ARM](case) for case in CASES]
result = {"pages": pages, "seconds": sum(p["seconds"] for p in pages)}
if ARM == "webclaw":
    t = time.perf_counter()
    for _ in CASES:
        subprocess.run([os.environ["WEBCLAW_BIN"], "--version"], capture_output=True, check=True)
    result["start_seconds"] = time.perf_counter() - t
OUT.write_text(json.dumps(result))
