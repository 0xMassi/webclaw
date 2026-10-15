"""Crawl coverage on sites whose links redirect, driven by ../crawl-redirects.sh. Runs under .work/venv-base.

usage: crawl_redirects.py <label> [rounds]
Each site is crawled one level deep from its seed. The ground truth is every same-host link on the seed page,
followed to its final URL, that robots.txt allows before and after the redirect. A link whose scheme fails is
tried again on the seed's scheme. An arm finds a page when it
returns content for that final URL, or for a link that redirects to it.
"""
import gzip, json, os, re, statistics, subprocess, sys, time, urllib.request
from pathlib import Path
from urllib.parse import urljoin, urlparse
from urllib.robotparser import RobotFileParser

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36"
SITES = [
    {"name": "quotes.toscrape.com", "seed": "https://quotes.toscrape.com/"},
    {"name": "quotes.toscrape.com-http", "seed": "http://quotes.toscrape.com/page/2/"},
    {"name": "www.python.org", "seed": "https://www.python.org/"},
    {"name": "go.dev", "seed": "https://go.dev/doc/"},
    {"name": "www.columbia.edu", "seed": "https://www.columbia.edu/~fdc/"},
]
MAX_PAGES, CONCURRENCY = 300, 4
ARMS = {"webclaw-before": "base", "webclaw": "base", "scrapling": "scrapling", "crawl4ai": "crawl4ai"}


def key(url):
    u = urlparse(url)
    return f"{u.netloc.lower()}{u.path.rstrip('/') or '/'}{'?' + u.query if u.query else ''}"


def get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=20) as r:
        body = r.read()
        if body[:2] == b"\x1f\x8b":
            body = gzip.decompress(body)
        return r.geturl(), body.decode("utf-8", "replace")


robots = {}


def allowed(url):
    u = urlparse(url)
    origin = f"{u.scheme}://{u.netloc}"
    if origin not in robots:
        judge = RobotFileParser()
        try:
            judge.parse(get(f"{origin}/robots.txt")[1].splitlines())
        except Exception:
            judge.parse([])
        robots[origin] = judge
    return robots[origin].can_fetch("*", url)


def truth(site):
    seed = site["seed"]
    host = urlparse(seed).hostname
    final_seed, html = get(seed)
    links = []
    for href in re.findall(r'<a\s[^>]*href=["\']([^"\'#]+)', html):
        url = urljoin(final_seed, href)
        if urlparse(url).scheme in ("http", "https") and urlparse(url).hostname == host and url not in links:
            links.append(url)
    moved, expected = {}, set()
    for url in links:
        try:
            final = get(url)[0]
        except Exception:
            try:
                final = get(urlparse(url)._replace(scheme=urlparse(final_seed).scheme).geturl())[0]
            except Exception:
                continue
        if urlparse(final).hostname != host or not allowed(url) or not allowed(final):
            continue
        if final != url:
            moved[key(url)] = key(final)
        expected.add(key(final))
    expected.discard(key(final_seed))
    return {"links": len(links), "redirecting": len(moved), "moved": moved, "expected": sorted(expected)}


def run_arm(arm, site, out):
    env = dict(os.environ)
    if arm == "webclaw-before":
        env["WEBCLAW_BIN"] = os.environ["WEBCLAW_BEFORE_BIN"]
    cmd = [str(WORK / f"venv-{ARMS[arm]}" / "bin/python"), str(ROOT / "scripts/crawl_arms.py"),
           arm.removesuffix("-before"), site["seed"], str(MAX_PAGES), "1", str(CONCURRENCY), str(out), "/", ""]
    out.unlink(missing_ok=True)
    start = time.time()
    r = subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if r.returncode:
        sys.exit(f"{arm} crawl of {site['name']} exited {r.returncode}\n{r.stderr[-2000:]}")
    return time.time() - start


def score(row, out, gt):
    pages = json.loads(out.read_text())["pages"] if out.exists() else []
    keys = [gt["moved"].get(key(p["url"]), key(p["url"])) for p in pages if p["ok"] and p["chars"]]
    expected = set(gt["expected"])
    row.update(pages=len(keys), found=len(expected & set(keys)), expected=len(expected),
               duplicates=len(keys) - len(set(keys)),
               missing=sorted(expected - set(keys))[:10])
    return row


def main(label, rounds):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    arms = [a for a in ARMS if a != "webclaw-before" or os.environ.get("WEBCLAW_BEFORE_BIN")]
    rows, truths = [], {}
    for site in SITES:
        gt = truths[site["name"]] = truth(site)
        if not gt["expected"]:
            sys.exit(f"no expected pages for {site['name']}; not scoring against an empty ground truth")
        print(json.dumps({"site": site["name"], "links": gt["links"], "redirecting": gt["redirecting"],
                          "expected": len(gt["expected"])}), flush=True)
        for rnd in range(rounds):
            for arm in arms:
                out = WORK / "crawl-redirects-out" / label / f"{site['name']}-{arm}-{rnd}.json"
                out.parent.mkdir(parents=True, exist_ok=True)
                row = {"site": site["name"], "arm": arm, "round": rnd, "seconds": round(run_arm(arm, site, out), 2)}
                rows.append(score(row, out, gt))
                print(json.dumps(row), flush=True)
    (res / "crawl-redirects-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    (res / "crawl-redirects-truth.json").write_text(json.dumps(truths, indent=1))
    lines = [f"# Crawl redirects benchmark {label}", "",
             f"Each arm crawls each seed one level deep (cap {MAX_PAGES} pages, concurrency {CONCURRENCY}), "
             "robots.txt obeyed. The expected pages are the seed page's same-host links, followed to their final "
             "URL, that robots.txt allows before and after the redirect. `found` counts expected pages the arm "
             "returned content for, under the final URL or a link that redirects to it. `duplicates` counts "
             "pages returned twice under such spellings. Medians over rounds.", "",
             "| site | links on seed | redirecting | arm | rounds | found | pages with content | duplicates | seconds |",
             "|---|---:|---:|---|---:|---:|---:|---:|---:|"]
    for site in SITES:
        gt = truths[site["name"]]
        for arm in arms:
            rs = [r for r in rows if r["site"] == site["name"] and r["arm"] == arm]
            med = lambda k: statistics.median(r[k] for r in rs)
            lines.append(f"| {site['name']} | {gt['links']} | {gt['redirecting']} | {arm} | {len(rs)} | "
                         f"{med('found'):.0f}/{len(gt['expected'])} | {med('pages'):.0f} | {med('duplicates'):.0f} | "
                         f"{med('seconds'):.1f} |")
    (res / "crawl-redirects-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main(sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 3)
