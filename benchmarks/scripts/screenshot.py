import json, os, statistics, subprocess, sys
from collections import Counter
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
URLS = [
    "https://example.com/",
    "https://quotes.toscrape.com/js/",
    "https://books.toscrape.com/",
    "https://en.wikipedia.org/wiki/Web_scraping",
    "https://news.ycombinator.com/",
    "https://www.rust-lang.org/",
    "https://docs.python.org/3/library/asyncio.html",
    "https://github.com/rust-lang/rust",
    "https://developer.mozilla.org/en-US/docs/Web/HTML",
    "https://www.bbc.com/news",
]
PYTHON = {"webclaw": {"cold": "venv-base", "warm": "venv-mcp"},
          "scrapling": {"cold": "venv-scrapling", "warm": "venv-mcp"},
          "crawl4ai": {"cold": "venv-crawl4ai", "warm": "venv-crawl4ai"}}
RUNS = [("cold", False), ("cold", True), ("warm", False)]
ONLY = os.environ.get("SCREENSHOT_ONLY")
WARM_ROUNDS = 2


def score(line):
    shot = json.loads(line)
    if not shot["file"]:
        return {**shot, "ok": False}
    path = Path(shot["file"])
    with Image.open(path) as img:
        colors = len(img.convert("RGB").resize((64, 64)).getcolors(4096) or [])
        shot.update(ok=colors > 3, format=img.format, width=img.width, height=img.height,
                    kb=path.stat().st_size / 1024, colors=colors)
    path.unlink()
    return shot


def run(arm, mode, full, out):
    python = WORK / PYTHON[arm][mode] / "bin/python"
    cmd = [str(python), str(ROOT / "scripts/screenshot_arms.py"), arm, mode, "1" if full else "0", str(out)]
    rounds = WARM_ROUNDS if mode == "warm" else 1
    shots = []
    for _ in range(rounds):
        r = subprocess.run(cmd + URLS, capture_output=True, text=True, timeout=1800)
        done = [score(l) for l in r.stdout.splitlines() if l.startswith("{")]
        seen = {s["url"] for s in done}
        missing = [u for u in URLS if u not in seen]
        if missing:
            print(f"{arm} {mode}: exit {r.returncode}, {len(missing)} pages missing\n{r.stderr[-2000:]}",
                  file=sys.stderr)
        shots += done + [{"url": u, "ok": False, "error": f"no result, exit {r.returncode}"} for u in missing]
    return shots


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows, heights = [], {}
    for mode, full in RUNS:
        for arm in PYTHON:
            if ONLY and ONLY != f"{arm}-{mode}-{int(full)}":
                continue
            shots = run(arm, mode, full, WORK / "screenshot-out" / f"{arm}-{mode}-{int(full)}")
            ok = [s for s in shots if s["ok"]]
            ms = sorted(s["ms"] for s in ok) or [0]
            sizes = Counter(f"{s['width']}x{s['height']}" for s in ok)
            row = {"arm": arm, "mode": mode, "full_page": full, "shots": len(shots), "ok": len(ok),
                   "ms": statistics.median(ms), "p95_ms": ms[int(0.95 * (len(ms) - 1))],
                   "kb": statistics.median([s["kb"] for s in ok] or [0]),
                   "size": sizes.most_common(1)[0][0] if sizes else "-",
                   "errors": [s["error"] for s in shots if not s["ok"]][:3]}
            rows.append(row)
            print(json.dumps(row), flush=True)
            if full:
                heights[arm] = {s["url"]: s.get("height") if s["ok"] else None for s in shots}
    (res / "screenshot-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# Screenshots {label}", "",
             f"Each tool screenshots {len(URLS)} pages as PNG. `cold` starts a fresh browser per page, as one command "
             "per screenshot does. `warm` keeps one browser open for every page: webclaw and Scrapling through their "
             "MCP servers' `screenshot` tools (Scrapling inside a browser session it opens first), Crawl4AI with one "
             f"crawler, {WARM_ROUNDS} rounds. `ok` counts images that are not blank (more than 3 colours in a 64x64 "
             "thumbnail). Crawl4AI's full-page capture stitches JPEG tiles.", "",
             "| tool | mode | full page | ok | ms (median) | ms (p95) | KB (median) | most common size |",
             "|---|---|---|---:|---:|---:|---:|---|"]
    for r in rows:
        lines.append(f"| {r['arm']} | {r['mode']} | {'yes' if r['full_page'] else 'no'} | {r['ok']}/{r['shots']} | "
                     f"{r['ms']:.0f} | {r['p95_ms']:.0f} | {r['kb']:.0f} | {r['size']} |")
    lines += ["", "Full-page height in pixels, per page:", "",
              "| page | " + " | ".join(heights) + " |", "|---|" + "---:|" * len(heights)]
    for url in URLS:
        lines.append(f"| {url} | " + " | ".join(str(heights[a].get(url) or "fail") for a in heights) + " |")
    (res / "screenshot-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


main(sys.argv[1])
