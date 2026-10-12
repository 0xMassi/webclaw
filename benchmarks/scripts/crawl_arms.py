"""One crawl by one tool. Runs inside that tool's venv.

usage: crawl_arms.py <arm> <seed> <max_pages> <max_depth> <concurrency> <out.json>
Writes {"pages": [{"url", "ok", "chars"}]} to out.json.
"""
import asyncio, json, os, subprocess, sys
from urllib.parse import urljoin, urlparse

ARM, SEED, MAX_PAGES, MAX_DEPTH, CONCURRENCY, OUT = sys.argv[1:7]
MAX_PAGES, MAX_DEPTH, CONCURRENCY = int(MAX_PAGES), int(MAX_DEPTH), int(CONCURRENCY)


def webclaw():
    r = subprocess.run([os.environ["WEBCLAW_BIN"], SEED, "--crawl", "--depth", str(MAX_DEPTH),
                        "--max-pages", str(MAX_PAGES), "--concurrency", str(CONCURRENCY), "--delay", "0",
                        "-f", "json"], capture_output=True, text=True)
    pages = json.loads(r.stdout or '{"pages": []}')["pages"]
    return [{"url": p["url"], "ok": p.get("extraction") is not None,
             "chars": len((p.get("extraction") or {}).get("content", {}).get("markdown", ""))} for p in pages]


def scrapling():
    from scrapling.spiders import Spider
    host = urlparse(SEED).hostname

    class Crawl(Spider):
        name = "bench"
        start_urls = [SEED]
        allowed_domains = {host}
        robots_txt_obey = True
        concurrent_requests = CONCURRENCY
        logging_level = 40

        def __init__(self):
            super().__init__()
            self.scheduled = 1
            self.depth = {SEED: 0}

        async def parse(self, response):
            yield {"url": response.url, "chars": len(response.markdown())}
            depth = self.depth.get(response.url, 0)
            if depth >= MAX_DEPTH:
                return
            for href in response.css("a::attr(href)").getall():
                url = urljoin(response.url, href).split("#")[0]
                if urlparse(url).hostname != host or url in self.depth or self.scheduled >= MAX_PAGES:
                    continue
                self.depth[url] = depth + 1
                self.scheduled += 1
                yield response.follow(url, callback=self.parse)

    result = Crawl().start()
    return [{"url": i["url"], "ok": True, "chars": i["chars"]} for i in result.items]


def crawl4ai():
    from crawl4ai import AsyncWebCrawler, CrawlerRunConfig, CacheMode
    from crawl4ai.deep_crawling import BFSDeepCrawlStrategy

    async def go():
        cfg = CrawlerRunConfig(
            deep_crawl_strategy=BFSDeepCrawlStrategy(max_depth=MAX_DEPTH, max_pages=MAX_PAGES, include_external=False),
            cache_mode=CacheMode.BYPASS, check_robots_txt=True, semaphore_count=CONCURRENCY, verbose=False)
        async with AsyncWebCrawler() as crawler:
            results = await crawler.arun(SEED, config=cfg)
        return [{"url": r.url, "ok": bool(r.success),
                 "chars": len(r.markdown.raw_markdown) if r.success and r.markdown else 0} for r in results]

    return asyncio.run(go())


pages = {"webclaw": webclaw, "scrapling": scrapling, "crawl4ai": crawl4ai}[ARM]()
json.dump({"pages": pages}, open(OUT, "w"))
