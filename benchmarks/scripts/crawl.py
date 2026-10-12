"""Crawl benchmark orchestrator, driven by ../crawl.sh. Runs under .work/venv-base (needs psutil).

usage: crawl.py <label> [rounds]
Every arm crawls every site in SITES with the same page cap, depth and concurrency, all obeying robots.txt.
"""
import json, os, statistics, subprocess, sys, time
from pathlib import Path
from urllib.parse import urlparse
from urllib.robotparser import RobotFileParser

import psutil

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
ARM_VENV = {"webclaw": "base", "scrapling": "scrapling", "crawl4ai": "crawl4ai"}
SITES = [
    {"name": "books.toscrape.com", "seed": "https://books.toscrape.com/", "max_pages": 300, "max_depth": 4},
    {"name": "scrapethissite.com", "seed": "https://www.scrapethissite.com/", "max_pages": 100, "max_depth": 3},
]
CONCURRENCY = 8


def norm(url):
    u = urlparse(url)
    return f"{u.scheme}://{u.netloc.lower()}{u.path.rstrip('/') or '/'}{'?' + u.query if u.query else ''}"


def run_arm(arm, site, out):
    py = str(WORK / f"venv-{ARM_VENV[arm]}" / "bin/python")
    cmd = [py, str(ROOT / "scripts/crawl_arms.py"), arm, site["seed"], str(site["max_pages"]),
           str(site["max_depth"]), str(CONCURRENCY), str(out)]
    start = time.time()
    proc = psutil.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    peak = 0
    while proc.poll() is None:
        try:
            tree = [proc] + proc.children(recursive=True)
            peak = max(peak, sum(p.memory_info().rss for p in tree if p.is_running()))
        except psutil.Error:
            pass
        time.sleep(0.2)
    return time.time() - start, peak / 2**20


def main(label, rounds):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rows = []
    for site in SITES:
        robots = RobotFileParser(f"https://{urlparse(site['seed']).netloc}/robots.txt")
        robots.read()
        for rnd in range(rounds):
            for arm in ARM_VENV:
                out = WORK / "crawl-out" / label / f"{site['name']}-{arm}-{rnd}.json"
                out.parent.mkdir(parents=True, exist_ok=True)
                secs, peak_mb = run_arm(arm, site, out)
                pages = json.loads(out.read_text())["pages"] if out.exists() else []
                urls = [norm(p["url"]) for p in pages]
                row = {"site": site["name"], "arm": arm, "round": rnd, "seconds": round(secs, 2),
                       "peak_mb": round(peak_mb), "pages": len(pages), "ok": sum(p["ok"] for p in pages),
                       "duplicates": len(urls) - len(set(urls)),
                       "disallowed": sum(p["ok"] and not robots.can_fetch("*", p["url"]) for p in pages)}
                rows.append(row)
                print(json.dumps(row), flush=True)
    (res / "crawl-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# Crawl benchmark {label}", "",
             f"Each arm crawls each site with the same page cap and depth, concurrency {CONCURRENCY}, "
             "robots.txt obeyed. Medians over rounds. `disallowed` counts fetched URLs that the site's "
             "robots.txt disallows and that returned content, judged by Python's urllib.robotparser.", "",
             "| site | arm | rounds | pages ok | seconds | pages/s | peak MB | duplicates | disallowed |",
             "|---|---|---:|---:|---:|---:|---:|---:|---:|"]
    for site in SITES:
        for arm in ARM_VENV:
            rs = [r for r in rows if r["site"] == site["name"] and r["arm"] == arm]
            med = lambda k: statistics.median(r[k] for r in rs)
            lines.append(f"| {site['name']} | {arm} | {len(rs)} | {med('ok'):.0f} | {med('seconds'):.1f} | "
                         f"{med('ok') / max(med('seconds'), 0.01):.1f} | {med('peak_mb'):.0f} | "
                         f"{med('duplicates'):.0f} | {max(r['disallowed'] for r in rs)} |")
    (res / "crawl-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main(sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 3)
