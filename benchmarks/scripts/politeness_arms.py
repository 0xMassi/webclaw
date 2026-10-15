"""One polite crawl by one tool. Runs inside that tool's venv.

usage: politeness_arms.py <arm> <seed> <max_pages> <concurrency> <delay-seconds> <out.json> [proxy]
Crawls the seed one level deep with robots.txt obeyed, asking each tool for `delay` seconds between requests
through its own politeness setting, and writes {"pages": [{"url", "ok"}]} to out.json. With a proxy, webclaw
reaches the seed through it.
"""
import asyncio, json, os, subprocess, sys
from urllib.parse import urljoin, urlparse

ARM, SEED, MAX_PAGES, CONCURRENCY, DELAY, OUT = sys.argv[1:7]
MAX_PAGES, CONCURRENCY, DELAY = int(MAX_PAGES), int(CONCURRENCY), float(DELAY)
PROXY = sys.argv[7] if len(sys.argv) > 7 else ""


def webclaw():
    cmd = [os.environ["WEBCLAW_BIN"], SEED, "--crawl", "--depth", "1", "--max-pages", str(MAX_PAGES),
           "--concurrency", str(CONCURRENCY), "--delay", str(round(DELAY * 1000)), "-f", "json"]
    if PROXY:
        cmd += ["--proxy", PROXY]
    r = subprocess.run(cmd, capture_output=True, text=True)
    pages = json.loads(r.stdout or '{"pages": []}')["pages"]
    return [{"url": p["url"], "ok": p.get("extraction") is not None} for p in pages]


def scrapling(per_domain):
    from scrapling.spiders import Spider
    netloc = urlparse(SEED).netloc

    class Crawl(Spider):
        name = "politeness"
        start_urls = [SEED]
        allowed_domains = {netloc}
        robots_txt_obey = True
        concurrent_requests = CONCURRENCY
        concurrent_requests_per_domain = 1 if per_domain else 0
        download_delay = DELAY
        logging_level = 40

        def __init__(self):
            super().__init__()
            self.seen = {SEED}

        async def parse(self, response):
            yield {"url": response.url}
            if response.request.url != SEED:
                return
            for href in response.css("a::attr(href)").getall():
                url = urljoin(response.url, href).split("#")[0]
                if urlparse(url).netloc != netloc or url in self.seen or len(self.seen) >= MAX_PAGES:
                    continue
                self.seen.add(url)
                yield response.follow(url, callback=self.parse)

    return [{"url": i["url"], "ok": True} for i in Crawl().start().items]


def crawl4ai():
    from crawl4ai import AsyncWebCrawler, CrawlerRunConfig, CacheMode
    from crawl4ai.deep_crawling import BFSDeepCrawlStrategy

    async def go():
        cfg = CrawlerRunConfig(
            deep_crawl_strategy=BFSDeepCrawlStrategy(max_depth=1, max_pages=MAX_PAGES, include_external=False),
            cache_mode=CacheMode.BYPASS, check_robots_txt=True, semaphore_count=CONCURRENCY,
            mean_delay=DELAY, max_range=0.0, verbose=False)
        async with AsyncWebCrawler() as crawler:
            results = await crawler.arun(SEED, config=cfg)
        return [{"url": r.url, "ok": bool(r.success)} for r in results]

    return asyncio.run(go())


arms = {"webclaw": webclaw, "scrapling": lambda: scrapling(False), "scrapling-per-domain": lambda: scrapling(True),
        "crawl4ai": crawl4ai}
json.dump({"pages": arms[ARM]()}, open(OUT, "w"))
