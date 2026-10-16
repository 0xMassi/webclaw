"""URL discovery: webclaw --map with and without the Common Crawl index against the other tools' URL seeders.

usage: url_seeding.py <label>
Each arm lists one site's URLs once. Its output is kept in .work/url-seeding-out/<label>/<site>/<arm>.json and
reused by a rerun with the same label, which then only rescores and never queries the index again; a new label (or
deleting that directory) measures again, so numbers from an older build never carry over. The arms without the index
run first; then every index arm of both tools runs in one stretch, site by site, alternating which tool goes first,
with a minute between index arms (URL_SEEDING_INDEX_GAP seconds). An index arm that reports an error (the index
answered 5xx, cut its answer short or dropped the connection) runs again up to three times, two minutes apart, so
an index outage is not scored as a tool result; what still fails is listed with its error. A seeded sample of each arm's URLs is fetched once to check it answers 200 with HTML; those checks
are kept in .work/url-seeding-out/live.json. Runs in the Scrapling venv for curl_cffi, which some sites (Hacker News)
need before they answer a script at all.
"""
import json, os, random, subprocess, sys, time
from pathlib import Path
from urllib.parse import urlsplit

from curl_cffi import requests

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
OUT = WORK / "url-seeding-out"
SITES = {
    "docs.python.org": "docs, a 7-URL sitemap",
    "www.sqlite.org": "docs, no sitemap",
    "go.dev": "docs, no sitemap",
    "kubernetes.io": "docs, a full sitemap",
    "news.ycombinator.com": "news, no sitemap",
    "lwn.net": "news, no sitemap",
    "books.toscrape.com": "shop, no sitemap",
    "www.allbirds.com": "shop, a full sitemap",
    "danluu.com": "blog, a full sitemap",
    "jvns.ca": "blog, a full sitemap",
}
WEBCLAW = {
    "webclaw": [],
    "webclaw-sitemap-only": ["--no-map-crawl"],
    "webclaw-cc": ["--map-source", "cc"],
    "webclaw-sitemap+cc": ["--map-source", "sitemap,cc"],
}
OTHERS = {
    "crawl4ai-sitemap": "venv-crawl4ai",
    "crawl4ai-cc": "venv-crawl4ai",
    "crawl4ai-sitemap+cc": "venv-crawl4ai",
    "scrapling-sitemap": "venv-scrapling",
}
INDEX_ARMS = {"webclaw-cc", "webclaw-sitemap+cc", "crawl4ai-cc", "crawl4ai-sitemap+cc"}
QUERIES = [
    ("docs.python.org", "how do I cancel an asyncio task", "docs.python.org/3/library/asyncio-task.html"),
    ("www.sqlite.org", "write-ahead log mode", "sqlite.org/wal.html"),
    ("go.dev", "tutorial on generics", "go.dev/doc/tutorial/generics"),
    ("kubernetes.io", "configure liveness probes",
     "kubernetes.io/docs/tasks/configure-pod-container/configure-liveness-readiness-startup-probes"),
]
SAMPLE = 20
REFUSED = {0, 403, 419, 429, 503}
ARM_TIMEOUT = 300
INDEX_GAP = int(os.environ.get("URL_SEEDING_INDEX_GAP", "60"))
INDEX_TRIES, INDEX_RETRY = 3, 120


def key(url):
    try:
        u = urlsplit(url.strip())
    except ValueError:
        return None
    if u.scheme not in ("http", "https") or not u.hostname:
        return None
    host = u.hostname.lower().removeprefix("www.")
    path = u.path if u.path in ("", "/") else u.path.rstrip("/")
    return f"{host}{path or '/'}" + (f"?{u.query}" if u.query else "")


def bare(host):
    return host.lower().removeprefix("www.")


def run_webclaw(arm, site, extra=()):
    cmd = [os.environ["WEBCLAW_BIN"], f"https://{site}/", "--map", "-f", "json", *WEBCLAW[arm], *extra]
    start = time.time()
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=ARM_TIMEOUT)
    except subprocess.TimeoutExpired:
        return {"urls": [], "seconds": ARM_TIMEOUT, "error": f"no answer in {ARM_TIMEOUT}s"}
    seconds = time.time() - start
    urls = [e["url"] for e in json.loads(r.stdout)] if r.stdout.strip().startswith("[") else []
    note = next((l for l in r.stderr.splitlines() if l.startswith(("warning:", "error:"))), None)
    return {"urls": urls, "seconds": seconds, "error": note if (note or r.returncode == 0) else r.stderr[-300:]}


def run_other(arm, site, path):
    python = WORK / OTHERS[arm] / "bin/python"
    try:
        subprocess.run([str(python), str(ROOT / "scripts/url_seeding_arms.py"), arm, site, str(path)],
                       capture_output=True, text=True, timeout=ARM_TIMEOUT)
    except subprocess.TimeoutExpired:
        return {"urls": [], "seconds": ARM_TIMEOUT, "error": f"no answer in {ARM_TIMEOUT}s"}
    return json.loads(path.read_text()) if path.exists() else {"urls": [], "seconds": 0, "error": "no output"}


def discover(run):
    found = {}
    plain = [a for a in [*WEBCLAW, *OTHERS] if a not in INDEX_ARMS]
    index = [("webclaw-cc", "crawl4ai-cc"), ("webclaw-sitemap+cc", "crawl4ai-sitemap+cc")]
    order = [(site, arm) for site in SITES for arm in plain]
    for i, site in enumerate(SITES):
        for pair in index:
            order += [(site, arm) for arm in (pair if i % 2 == 0 else pair[::-1])]
    for site, arm in order:
        path = run / site / f"{arm}.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            for attempt in range(1, INDEX_TRIES + 1):
                print(f"{site} {arm} ...", file=sys.stderr, flush=True)
                result = run_webclaw(arm, site) if arm in WEBCLAW else run_other(arm, site, path)
                result["attempts"] = attempt
                if arm not in INDEX_ARMS:
                    break
                time.sleep(INDEX_GAP)
                if not result.get("error") or attempt == INDEX_TRIES:
                    break
                print(f"{site} {arm}: the index failed ({result['error'][:100]}), again in {INDEX_RETRY}s",
                      file=sys.stderr, flush=True)
                time.sleep(INDEX_RETRY)
            path.write_text(json.dumps(result))
        found[site, arm] = json.loads(path.read_text())
    return found


def check(url):
    try:
        r = requests.get(url, impersonate="chrome", timeout=20, allow_redirects=True, stream=True)
        result = {"status": r.status_code, "html": "html" in r.headers.get("content-type", "").lower(),
                  "redirected": bool(r.redirect_count)}
        r.close()
        return result
    except Exception as e:
        return {"status": 0, "html": False, "redirected": False, "error": type(e).__name__}


def round_robin(urls):
    by_host = {}
    for url in urls:
        by_host.setdefault(urlsplit(url).hostname, []).append(url)
    queues = list(by_host.values())
    return [q[i] for i in range(max(map(len, queues), default=0)) for q in queues if i < len(q)]


def verify(found):
    cache_path = OUT / "live.json"
    cache = json.loads(cache_path.read_text()) if cache_path.exists() else {}
    cache = {u: c for u, c in cache.items() if c["status"] not in REFUSED}
    samples = {}
    for (site, arm), result in found.items():
        scoped = sorted({u for u in result["urls"] if in_scope(u, site)})
        samples[site, arm] = random.Random(f"{site}|{arm}").sample(scoped, min(SAMPLE, len(scoped)))
    for pause in (1, 10):
        todo = round_robin(sorted({u for s in samples.values() for u in s
                                   if u not in cache or cache[u]["status"] in REFUSED}))
        for i, url in enumerate(todo):
            cache[url] = check(url)
            time.sleep(pause)
            if i % 50 == 49:
                cache_path.write_text(json.dumps(cache))
                print(f"checked {i + 1}/{len(todo)}", file=sys.stderr, flush=True)
    cache_path.write_text(json.dumps(cache))
    return samples, cache


def in_scope(url, site):
    try:
        host = urlsplit(url).hostname
    except ValueError:
        return False
    return bool(host) and bare(host) == bare(site)


def queries(run, found):
    rows = []
    for site, question, target in QUERIES:
        path = run / site / "query.json"
        if not path.exists():
            plain = [key(u) for u in found[site, "webclaw-sitemap+cc"]["urls"]]
            ranks = {"plain": plain.index(target) + 1 if target in plain else None, "plain_urls": len(plain)}
            cmd = [os.environ["WEBCLAW_BIN"], f"https://{site}/", "--map", "-f", "json",
                   "--map-source", "sitemap,cc", "--query", question]
            try:
                r = subprocess.run(cmd, capture_output=True, text=True, timeout=ARM_TIMEOUT)
            except subprocess.TimeoutExpired:
                r = subprocess.CompletedProcess(cmd, 1, "", "")
            keys = [key(e["url"]) for e in json.loads(r.stdout)] if r.stdout.strip().startswith("[") else []
            ranks["query"] = keys.index(target) + 1 if target in keys else None
            ranks["query_urls"] = len(keys)
            time.sleep(INDEX_GAP)
            path.write_text(json.dumps(ranks))
        rows.append({"site": site, "question": question, "target": target, **json.loads(path.read_text())})
    return rows


def score(found, samples, cache):
    rows = []
    for site in SITES:
        union = set()
        for arm in [*WEBCLAW, *OTHERS]:
            union |= {key(u) for u in found[site, arm]["urls"] if in_scope(u, site)} - {None}
        for arm in [*WEBCLAW, *OTHERS]:
            result = found[site, arm]
            urls = result["urls"]
            keys = [key(u) for u in urls]
            unique = set(keys) - {None}
            scoped = {key(u) for u in urls if in_scope(u, site)} - {None}
            checked = [cache[u] for u in samples[site, arm] if cache[u]["status"] not in REFUSED]
            live = sum(c["status"] == 200 and c["html"] for c in checked)
            live_share = live / len(checked) if checked else None
            rows.append({"site": site, "arm": arm, "urls": len(urls), "unique": len(unique),
                         "duplicates": len(urls) - len(unique), "in_scope": len(scoped),
                         "with_query": sum("?" in k for k in scoped),
                         "sampled": len(checked), "live": live, "live_share": live_share,
                         "redirected": sum(c["redirected"] for c in checked),
                         "est_live": round(len(scoped) * live_share) if checked else 0,
                         "union_share": len(scoped) / len(union) if union else 0,
                         "seconds": result["seconds"], "error": result.get("error"),
                         "attempts": result.get("attempts", 1)})
    return rows


def pct(x):
    return "-" if x is None else f"{100 * x:.0f}%"


def summary(label, rows, query_rows):
    arms = [*WEBCLAW, *OTHERS]
    lines = [f"# URL discovery {label}", "",
             f"Each arm lists the URLs of {len(SITES)} sites once, starting from `https://<site>/`. `in scope` counts "
             "distinct URLs on the site's own host (`www.` ignored), compared without scheme, fragment or trailing "
             f"slash. `live` is the share of a seeded sample of up to {SAMPLE} in-scope URLs per arm that answered 200 "
             "with HTML after redirects, fetched with curl_cffi impersonating Chrome, one request a second across sites "
             "taken in turn; answers that refuse the checker (403, 419, 429, 503 or no answer) are fetched again ten "
             "seconds apart, and those still refused are left out of the share. `est. live` is in scope times live. `of union` is the arm's share of every "
             "in-scope URL any arm found on that site. `dups` counts URLs returned more than once (same comparison). "
             "`with a query string` counts in-scope URLs with a `?` part, which in the index are often the same page "
             "with a tracking or search parameter. "
             "Times are wall clock for one cold run; the Common Crawl index is a shared public service and its "
             "answer times vary from run to run, so the index arms of both tools run in one stretch, site by site, "
             f"alternating which tool goes first. An arm that gives no answer in {ARM_TIMEOUT} s is stopped.", "",
             "Totals over all sites:", "",
             "| arm | URLs | dups | in scope | with a query string | live (sampled) | est. live | of union (median) | seconds (median) | errors |",
             "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for arm in arms:
        rs = [r for r in rows if r["arm"] == arm]
        sampled, live = sum(r["sampled"] for r in rs), sum(r["live"] for r in rs)
        shares = sorted(r["union_share"] for r in rs)
        secs = sorted(r["seconds"] for r in rs)
        lines.append(f"| {arm} | {sum(r['urls'] for r in rs):,} | {sum(r['duplicates'] for r in rs):,} | "
                     f"{sum(r['in_scope'] for r in rs):,} | {sum(r['with_query'] for r in rs):,} | "
                     f"{pct(live / sampled if sampled else None)} ({sampled}) | "
                     f"{sum(r['est_live'] for r in rs):,} | {pct(shares[len(shares) // 2])} | "
                     f"{secs[len(secs) // 2]:.1f} | {sum(bool(r['error']) for r in rs)} |")
    lines += ["", "In-scope URLs per site (live share of the sample in brackets):", "",
              "| site | kind | " + " | ".join(arms) + " |", "|---|---|" + "---:|" * len(arms)]
    for site, kind in SITES.items():
        cells = []
        for arm in arms:
            r = next(r for r in rows if r["site"] == site and r["arm"] == arm)
            cells.append(f"{r['in_scope']:,} ({pct(r['live_share'])})" + (" E" if r["error"] else ""))
        lines.append(f"| {site} | {kind} | " + " | ".join(cells) + " |")
    lines += ["", "`E`: the arm reported an error (below). Seconds per site:", "",
              "| site | " + " | ".join(arms) + " |", "|---|" + "---:|" * len(arms)]
    for site in SITES:
        lines.append(f"| {site} | " + " | ".join(
            f"{next(r for r in rows if r['site'] == site and r['arm'] == arm)['seconds']:.1f}" for arm in arms) + " |")
    errors = [r for r in rows if r["error"]]
    if errors:
        lines += ["", "Errors (an index arm that failed is run again up to "
                  f"{INDEX_TRIES} times, {INDEX_RETRY} s apart; the last error is kept):", ""]
        lines += [f"- {r['site']} {r['arm']}, {r['attempts']} tries: {r['error']}" for r in errors]
    retried = [r for r in rows if r["attempts"] > 1 and not r["error"]]
    if retried:
        lines += ["", "Index arms that succeeded only on a later try:", ""]
        lines += [f"- {r['site']} {r['arm']}: try {r['attempts']}" for r in retried]
    lines += ["", "Rank of the page that answers a question in `webclaw --map --map-source sitemap,cc`, without "
              "`--query` (the arm above) and with it (a second run, so the index may answer differently; Crawl4AI "
              "ranks by a query only after fetching the `<head>` of every URL, so it has no row here):", "",
              "| site | question | target | URLs without | rank without | URLs with | rank with --query |",
              "|---|---|---|---:|---:|---:|---:|"]
    for q in query_rows:
        lines.append(f"| {q['site']} | {q['question']} | {q['target']} | {q.get('plain_urls', 0):,} | "
                     f"{q.get('plain') or 'absent'} | {q.get('query_urls', 0):,} | {q.get('query') or 'absent'} |")
    return "\n".join(lines) + "\n"


def main(label):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    run = OUT / label
    found = discover(run)
    query_rows = queries(run, found)
    samples, cache = verify(found)
    rows = score(found, samples, cache)
    (res / "url-seeding-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows + query_rows))
    text = summary(label, rows, query_rows)
    (res / "url-seeding-summary.md").write_text(text)
    print(text)


main(sys.argv[1])
