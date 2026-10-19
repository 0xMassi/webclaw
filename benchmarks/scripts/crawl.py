"""Crawl benchmark orchestrator, driven by ../crawl.sh. Runs under .work/venv-base (needs psutil).

usage: crawl.py <label> [rounds]      crawl, then score
       crawl.py <label> --rescore     score the saved outputs again
Every arm crawls every site in SITES with the same page cap, depth and concurrency, all obeying robots.txt.
"""
import json, os, re, statistics, subprocess, sys, time
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
    {"name": "quotes.toscrape.com-js", "seed": "https://quotes.toscrape.com/js/", "max_pages": 20, "max_depth": 12,
     "prefix": "/js/", "js": True, "expect": [f"https://quotes.toscrape.com/js/page/{n}/" for n in range(1, 11)]},
]
CONCURRENCY = 8
RESUME_SITE = {"name": "books.toscrape.com", "seed": "https://books.toscrape.com/", "max_pages": 200, "max_depth": 4}
KILL_AFTER = 3.0


def norm(url):
    u = urlparse(url)
    return f"{u.scheme}://{u.netloc.lower()}{u.path.rstrip('/') or '/'}{'?' + u.query if u.query else ''}"


ARM_TIMEOUT = 1800
FETCH_TIMEOUT = 120


def run_arm(arm, site, out):
    py = str(WORK / f"venv-{ARM_VENV[arm]}" / "bin/python")
    cmd = [py, str(ROOT / "scripts/crawl_arms.py"), arm, site["seed"], str(site["max_pages"]),
           str(site["max_depth"]), str(CONCURRENCY), str(out), site.get("prefix", "/"), "js" if site.get("js") else ""]
    start = time.time()
    out.unlink(missing_ok=True)
    proc = psutil.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    peak = 0
    deadline = start + ARM_TIMEOUT
    while proc.poll() is None:
        if time.time() > deadline:
            for p in proc.children(recursive=True) + [proc]:
                try:
                    p.kill()
                except psutil.Error:
                    pass
            sys.exit(f"{arm} crawl of {site['name']} ran past {ARM_TIMEOUT} s")
        try:
            tree = [proc] + proc.children(recursive=True)
            peak = max(peak, sum(p.memory_info().rss for p in tree if p.is_running()))
        except psutil.Error:
            pass
        time.sleep(0.2)
    if proc.returncode:
        sys.exit(f"{arm} crawl of {site['name']} exited {proc.returncode}")
    return time.time() - start, peak / 2**20


def robots_judge(site):
    origin = f"https://{urlparse(site['seed']).netloc}"
    try:
        r = subprocess.run([os.environ["WEBCLAW_BIN"], f"{origin}/robots.txt", "--raw-html"],
                           capture_output=True, text=True, timeout=FETCH_TIMEOUT)
        lines = r.stdout.splitlines() if r.returncode == 0 else []
    except subprocess.TimeoutExpired:
        lines = []
    judge = RobotFileParser()
    judge.parse(lines)
    return judge


def letters(text):
    return re.sub(r"[^a-z0-9]", "", text.lower())


def expected_items(site):
    if "expect" not in site:
        return []
    items = []
    for url in site["expect"]:
        try:
            r = subprocess.run([os.environ["WEBCLAW_BIN"], url, "--raw-html"], capture_output=True, text=True,
                               timeout=FETCH_TIMEOUT)
        except subprocess.TimeoutExpired:
            sys.exit(f"no reference items from {url} (timed out); not scoring an incomplete set")
        found = [letters(json.loads(f'"{t}"'))[:60] for t in re.findall(r'"text": "((?:[^"\\]|\\.)*)"', r.stdout)]
        if r.returncode or not found:
            sys.exit(f"no reference items from {url} (exit {r.returncode}); not scoring an incomplete set")
        items += found
    return items


def score(row, out, judge, items):
    pages = json.loads(out.read_text())["pages"] if out.exists() else []
    urls = [norm(p["url"]) for p in pages]
    row.update(pages=len(pages), ok=sum(p["ok"] for p in pages), duplicates=len(urls) - len(set(urls)),
               disallowed=sum(p["ok"] and not judge.can_fetch("*", p["url"]) for p in pages))
    if items:
        text = letters(" ".join(p.get("markdown", "") for p in pages))
        row.update(found=sum(item in text for item in items), expected=len(items))
    return row


def main(label, rounds):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    rescore = rounds is None
    saved = {(r["site"], r["arm"], r["round"]): r for r in map(json.loads, open(res / "crawl-rows.jsonl"))} if rescore else {}
    rows = []
    sites = [s for s in SITES if not rescore or any(k[0] == s["name"] for k in saved)]
    for site in sites:
        judge = robots_judge(site)
        items = expected_items(site)
        for rnd in range(rounds or 1 + max(k[2] for k in saved)):
            for arm in ARM_VENV:
                out = WORK / "crawl-out" / label / f"{site['name']}-{arm}-{rnd}.json"
                if rescore:
                    row = saved[(site["name"], arm, rnd)]
                else:
                    out.parent.mkdir(parents=True, exist_ok=True)
                    secs, peak_mb = run_arm(arm, site, out)
                    row = {"site": site["name"], "arm": arm, "round": rnd, "seconds": round(secs, 2),
                           "peak_mb": round(peak_mb)}
                rows.append(score(row, out, judge, items))
                print(json.dumps(row), flush=True)
    (res / "crawl-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# Crawl benchmark {label}", "",
             f"Each arm crawls each site with the same page cap and depth, concurrency {CONCURRENCY}, "
             "robots.txt obeyed. Medians over rounds. `disallowed` counts fetched URLs that the site's "
             "robots.txt disallows and that returned content, judged by Python's urllib.robotparser.", "",
             "`pages ok` can exceed the cap when a tool does not enforce it. `content found` counts, on sites "
             "that build their content with JavaScript, the expected items (quotes) present in the crawl's "
             "markdown.", "",
             "| site | arm | rounds | pages ok | content found | seconds | pages/s | peak MB | duplicates | disallowed |",
             "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for site in sites:
        for arm in ARM_VENV:
            rs = [r for r in rows if r["site"] == site["name"] and r["arm"] == arm]
            med = lambda k: statistics.median(r[k] for r in rs)
            found = f"{med('found'):.0f}/{rs[0]['expected']}" if "found" in rs[0] else "-"
            lines.append(f"| {site['name']} | {arm} | {len(rs)} | {med('ok'):.0f} | {found} | {med('seconds'):.1f} | "
                         f"{med('ok') / max(med('seconds'), 0.01):.1f} | {med('peak_mb'):.0f} | "
                         f"{med('duplicates'):.0f} | {max(r['disallowed'] for r in rs)} |")
    if not rescore:
        lines += resume_test(label)
    (res / "crawl-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


def kill_tree(proc):
    for p in proc.children(recursive=True) + [proc]:
        try:
            p.kill()
        except psutil.Error:
            pass
    proc.wait()


def resume_test(label):
    site = RESUME_SITE
    rows = []
    for arm in ARM_VENV:
        work = WORK / "resume" / label / arm
        if work.exists():
            for f in sorted(work.rglob("*"), reverse=True):
                f.unlink() if f.is_file() else f.rmdir()
        work.mkdir(parents=True, exist_ok=True)
        py = str(WORK / f"venv-{ARM_VENV[arm]}" / "bin/python")
        cmd = [py, str(ROOT / "scripts/resume_arms.py"), arm, site["seed"], str(site["max_pages"]),
               str(site["max_depth"]), str(CONCURRENCY), str(work)]
        start = time.time()
        proc = psutil.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(KILL_AFTER)
        kill_tree(proc)
        out = work / "out.jsonl"
        before_kill = len(out.read_text().splitlines()) if out.exists() else 0
        restarts, finished = 0, False
        while restarts < 3 and not finished:
            restarts += 1
            finished = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
        seconds = time.time() - start - KILL_AFTER
        pages = [json.loads(l) for l in out.read_text().splitlines()] if out.exists() else []
        urls = [norm(p["url"]) for p in pages if p["ok"]]
        rows.append({"arm": arm, "before_kill": before_kill, "unique": len(set(urls)),
                     "duplicates": len(urls) - len(set(urls)), "seconds": round(seconds, 1), "restarts": restarts,
                     "finished": finished})
        print(json.dumps(rows[-1]), flush=True)
    (ROOT / "results" / label / "resume-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    return ["", "## Kill and resume", "",
            f"Each tool crawls {site['name']} (cap {site['max_pages']}, depth {site['max_depth']}, concurrency "
            f"{CONCURRENCY}), streaming pages to a file, through its own resume mechanism. After {KILL_AFTER:.0f}s "
            "the whole process tree gets SIGKILL, then the same command reruns until it exits cleanly. "
            "`unique` counts distinct pages in the output; `duplicates` counts pages written twice.", "",
            "| arm | pages before kill | unique pages at end | duplicates | seconds after restart | runs after kill |",
            "|---|---:|---:|---:|---:|---:|"] + [
        f"| {r['arm']} | {r['before_kill']} | {r['unique']} | {r['duplicates']} | {r['seconds']} | "
        f"{r['restarts'] if r['finished'] else 'did not finish'} |"
        for r in rows]


if __name__ == "__main__":
    arg = sys.argv[2] if len(sys.argv) > 2 else "3"
    main(sys.argv[1], None if arg == "--rescore" else int(arg))
