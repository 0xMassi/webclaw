"""Crawl politeness against a local recording server, driven by ../politeness.sh. Runs under .work/venv-base.

usage: politeness.py <label> [rounds]
Each arm crawls a 30-page site one level deep, concurrency 4, robots.txt obeyed, configured for the same spacing
between requests. The site is a server on 127.0.0.1 that answers every page after 100 ms and logs when each
request arrives, so no real site is hit. webclaw refuses private addresses, so it reaches the same server as an
HTTP proxy for http://example.com/; the other tools fetch 127.0.0.1 directly. Only page requests count:
robots.txt and favicon requests are left out.
"""
import json, os, statistics, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".work"
PAGES, CONCURRENCY, LATENCY = 30, 4, 0.1
SCENARIOS = [
    {"name": "delay 500 ms", "delay": 0.5, "robots": "User-agent: *\nAllow: /\n", "spacing": 0.5},
    {"name": "delay 200 ms, Crawl-delay 1", "delay": 0.2, "robots": "User-agent: *\nCrawl-delay: 1\n",
     "spacing": 1.0, "pages": 12},
]
ARMS = {"webclaw-before": "base", "webclaw": "base", "scrapling": "scrapling",
        "scrapling-per-domain": "scrapling", "crawl4ai": "crawl4ai"}
LABELS = {"webclaw-before": "webclaw before (`--delay`)", "webclaw": "webclaw (`--delay`)",
          "scrapling": "Scrapling spider (`download_delay`)",
          "scrapling-per-domain": "Scrapling spider (`download_delay`, `concurrent_requests_per_domain=1`)",
          "crawl4ai": "Crawl4AI BFS deep crawl (`mean_delay`, `max_range=0`)"}

log, robots = [], [""]
lock = threading.Lock()


class Site(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        at = time.monotonic()
        target = urlparse(self.path)
        host = target.netloc or self.headers.get("Host", "")
        path = target.path or "/"
        with lock:
            log.append((host, path, at))
        time.sleep(LATENCY)
        status, body = 200, ""
        if path == "/robots.txt":
            body = robots[0]
        elif path == "/":
            links = "".join(f'<li><a href="/p/{i}">Page {i}</a></li>' for i in range(1, PAGES))
            body = f"<html><head><title>Seed</title></head><body><main><h1>Seed</h1><p>A small site.</p><ul>{links}</ul></main></body></html>"
        elif path.startswith("/p/"):
            body = (f"<html><head><title>Page {path}</title></head><body><main><h1>Page {path}</h1>"
                    f"<p>This page exists so a crawler has something to read. It is page {path}.</p></main></body></html>")
        else:
            status, body = 404, "not found"
        data = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", "text/plain" if path == "/robots.txt" else "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def run_arm(arm, scenario, port, out):
    env = dict(os.environ)
    if arm == "webclaw-before":
        env["WEBCLAW_BIN"] = os.environ["WEBCLAW_BEFORE_BIN"]
    via_proxy = arm.startswith("webclaw")
    seed = "http://example.com/" if via_proxy else f"http://127.0.0.1:{port}/"
    cmd = [str(WORK / f"venv-{ARMS[arm]}" / "bin/python"), str(ROOT / "scripts/politeness_arms.py"),
           arm.removesuffix("-before"), seed, str(scenario.get("pages", PAGES)), str(CONCURRENCY),
           str(scenario["delay"]), str(out)] + ([f"http://127.0.0.1:{port}"] if via_proxy else [])
    out.unlink(missing_ok=True)
    with lock:
        log.clear()
    start = time.time()
    r = subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    seconds = time.time() - start
    if r.returncode:
        sys.exit(f"{arm} on {scenario['name']} exited {r.returncode}\n{r.stderr[-2000:]}")
    with lock:
        return seconds, list(log)


def score(row, hits, out, spacing):
    pages = [(h, at) for h, path, at in hits if path not in ("/robots.txt", "/favicon.ico")]
    hosts = sorted({h for h, _ in pages})
    gaps = []
    for host in hosts:
        starts = sorted(at for h, at in pages if h == host)
        gaps += [b - a for a, b in zip(starts, starts[1:])]
    times = sorted(at for _, at in pages)
    span = times[-1] - times[0] if len(times) > 1 else 0.0
    ok = sum(p["ok"] for p in json.loads(out.read_text())["pages"]) if out.exists() else 0
    row.update(hosts=len(hosts), requests=len(pages), ok_pages=ok,
               rate=round((len(pages) - 1) / span, 2) if span else 0.0,
               min_gap_ms=round(min(gaps) * 1000) if gaps else None,
               median_gap_ms=round(statistics.median(gaps) * 1000) if gaps else None,
               too_close=sum(g < 0.9 * spacing for g in gaps), gaps=len(gaps))
    return row


def main(label, rounds):
    res = ROOT / "results" / label
    res.mkdir(parents=True, exist_ok=True)
    arms = [a for a in ARMS if a != "webclaw-before" or os.environ.get("WEBCLAW_BEFORE_BIN")]
    server = ThreadingHTTPServer(("127.0.0.1", 0), Site)
    server.daemon_threads = True
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    rows = []
    for rnd in range(rounds):
        for scenario in SCENARIOS:
            robots[0] = scenario["robots"]
            for arm in arms:
                out = WORK / "politeness-out" / label / f"{scenario['delay']}-{arm}-{rnd}.json"
                out.parent.mkdir(parents=True, exist_ok=True)
                seconds, hits = run_arm(arm, scenario, port, out)
                row = {"scenario": scenario["name"], "arm": arm, "round": rnd, "seconds": round(seconds, 2)}
                rows.append(score(row, hits, out, scenario["spacing"]))
                print(json.dumps(row), flush=True)
    server.shutdown()
    (res / "politeness-rows.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    lines = [f"# Crawl politeness benchmark {label}", "",
             f"Each arm crawls a local site one level deep (concurrency {CONCURRENCY}, robots.txt obeyed), set for the "
             f"spacing in the scenario. The site answers each page after {int(LATENCY * 1000)} ms and logs when each "
             "request arrives. `req/s` is page requests to the host divided by the time from the first to the last; "
             "the spacing allows at most 1/spacing. `too close` counts consecutive requests to the host that arrived "
             "less than 90% of the spacing apart. `seconds` is the whole run, process start included. Medians over "
             "rounds.", "",
             "| scenario | spacing | arm | rounds | pages ok | requests | req/s | shortest gap ms | median gap ms | too close | seconds |",
             "|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for scenario in SCENARIOS:
        for arm in arms:
            rs = [r for r in rows if r["scenario"] == scenario["name"] and r["arm"] == arm]
            med = lambda k: statistics.median(r[k] for r in rs if r[k] is not None)
            lines.append(f"| {scenario['name']} | {scenario['spacing']:.1f} s | {LABELS[arm]} | {len(rs)} | "
                         f"{med('ok_pages'):.0f} | {med('requests'):.0f} | {med('rate'):.2f} | {med('min_gap_ms'):.0f} | "
                         f"{med('median_gap_ms'):.0f} | {med('too_close'):.0f}/{med('gaps'):.0f} | {med('seconds'):.1f} |")
    (res / "politeness-summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main(sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 2)
