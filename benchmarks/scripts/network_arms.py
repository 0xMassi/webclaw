"""One tool capturing the network responses of a list of pages, a fresh browser per page.

usage: network_arms.py <arm> <filtered:0|1> <cases.json>
Prints one JSON line per page: {"page", "ms", "bytes", "error", "responses": [{"url", "status", "body"}]}.
`bytes` is the size of what the tool hands back, serialized as JSON. With filtered=1 each tool keeps only
the URLs matching the page's ground-truth pattern, where it can: webclaw with `--capture-network=PATTERN`,
Scrapling with `capture_xhr=PATTERN`. Crawl4AI's `capture_network_requests` has no filter.
"""
import asyncio, json, os, subprocess, sys, time

ARM, FILTERED, CASES = sys.argv[1], sys.argv[2] == "1", json.load(open(sys.argv[3]))


def pattern(case):
    return "|".join(api["match"] for api in case["apis"]) if FILTERED else None


def emit(case, t0, raw, responses, error=None):
    print(json.dumps({"page": case["page"], "ms": 1000 * (time.perf_counter() - t0), "bytes": len(raw or b""),
                      "error": error, "responses": responses}), flush=True)


def text(body):
    if body is None:
        return None
    if isinstance(body, bytes):
        return body.decode("utf-8", "replace")
    return body


def webclaw():
    for case in CASES:
        flag = f"--capture-network={pattern(case)}" if FILTERED else "--capture-network"
        cmd = [os.environ["WEBCLAW_BIN"], case["page"], flag]
        t0 = time.perf_counter()
        r = subprocess.run(cmd, capture_output=True, timeout=180)
        if r.returncode != 0:
            emit(case, t0, b"", [], r.stderr.decode()[-300:])
            continue
        captured = json.loads(r.stdout)
        emit(case, t0, r.stdout, [{"url": c["url"], "status": c["status"], "body": c["body"]} for c in captured])


def scrapling():
    from scrapling.fetchers import DynamicFetcher
    for case in CASES:
        t0 = time.perf_counter()
        try:
            page = DynamicFetcher.fetch(case["page"], headless=True, network_idle=True,
                                        capture_xhr=pattern(case) or ".")
            captured = [{"url": x.url, "status": x.status, "headers": dict(x.headers),
                         "request_headers": dict(x.request_headers), "body": text(x.body)}
                        for x in page.captured_xhr]
            raw = json.dumps(captured).encode()
            emit(case, t0, raw, [{"url": c["url"], "status": c["status"], "body": c["body"]} for c in captured])
        except Exception as e:
            emit(case, t0, b"", [], f"{type(e).__name__}: {e}"[-300:])


async def crawl4ai():
    from crawl4ai import AsyncWebCrawler, BrowserConfig, CacheMode, CrawlerRunConfig
    for case in CASES:
        t0 = time.perf_counter()
        try:
            async with AsyncWebCrawler(config=BrowserConfig(headless=True, verbose=False)) as crawler:
                r = await crawler.arun(case["page"], config=CrawlerRunConfig(
                    capture_network_requests=True, wait_until="networkidle", cache_mode=CacheMode.BYPASS,
                    verbose=False, page_timeout=60000))
            if not r.success:
                raise RuntimeError(r.error_message or "crawl failed")
            events = r.network_requests or []
            raw = json.dumps(events, default=str).encode()
            emit(case, t0, raw, [{"url": e["url"], "status": e.get("status"), "body": text((e.get("body") or {}).get("text"))}
                                 for e in events if e.get("event_type") == "response"])
        except Exception as e:
            emit(case, t0, b"", [], f"{type(e).__name__}: {e}"[-300:])


if ARM == "webclaw":
    webclaw()
elif ARM == "scrapling":
    scrapling()
else:
    asyncio.run(crawl4ai())
