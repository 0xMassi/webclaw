"""One tool fetching a list of pages through its own response cache.

usage: cache_arms.py <arm> <cache-dir> <cold|warm> <out.json> <url>...
Writes {"seconds", "pages": {url: text}}. The warm run sends every request to a proxy that refuses
connections, so a page only comes back if the tool answered it from its cache.
"""
import asyncio, json, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ARM, CACHE, RUN, OUT, URLS = sys.argv[1], sys.argv[2], sys.argv[3], Path(sys.argv[4]), sys.argv[5:]
DEAD_PROXY = "http://127.0.0.1:9"
TIMEOUT = 30


def webclaw():
    env = {**os.environ, "WEBCLAW_CACHE_DIR": CACHE}
    proxy = ["-p", DEAD_PROXY] if RUN == "warm" else []

    def one(url):
        r = subprocess.run([os.environ["WEBCLAW_BIN"], url, "-f", "llm", "--render", "never", "--cache",
                            "-t", str(TIMEOUT), *proxy], capture_output=True, text=True, env=env,
                           timeout=TIMEOUT + 15)
        return url, r.stdout if r.returncode == 0 else ""

    with ThreadPoolExecutor(8) as pool:
        return dict(pool.map(one, URLS))


def scrapling():
    from scrapling.fetchers import FetcherSession
    from scrapling.spiders import Spider

    class Pages(Spider):
        name = "cache"
        start_urls = URLS
        concurrent_requests = 8
        logging_level = 50
        development_mode = True
        development_cache_dir = CACHE

        def configure_sessions(self, manager):
            if RUN == "warm":
                manager.add("default", FetcherSession(proxy=DEAD_PROXY, retries=1))
            else:
                super().configure_sessions(manager)

        async def parse(self, response):
            yield {"url": str(response.request.url) if getattr(response, "request", None) else response.url,
                   "status": response.status, "markdown": response.markdown()}

    items = Pages().start().items
    return {i["url"]: i["markdown"] if i["status"] < 400 else "" for i in items}


def crawl4ai():
    os.environ["CRAWL4_AI_BASE_DIRECTORY"] = CACHE
    from crawl4ai import AsyncWebCrawler, BrowserConfig, CacheMode, CrawlerRunConfig

    async def go():
        browser = BrowserConfig(headless=True, verbose=False, proxy=DEAD_PROXY if RUN == "warm" else None)
        cfg = CrawlerRunConfig(cache_mode=CacheMode.ENABLED, page_timeout=TIMEOUT * 1000, verbose=False)
        async with AsyncWebCrawler(config=browser) as crawler:
            results = await crawler.arun_many(URLS, config=cfg)
        return {r.url: (r.markdown.raw_markdown if r.success and r.markdown else "") for r in results}

    return asyncio.run(go())


start = time.perf_counter()
pages = {"webclaw": webclaw, "scrapling": scrapling, "crawl4ai": crawl4ai}[ARM]()
OUT.write_text(json.dumps({"seconds": time.perf_counter() - start, "pages": pages}))
