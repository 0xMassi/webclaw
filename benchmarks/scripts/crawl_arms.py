"""One crawl by one tool. Runs inside that tool's venv.

usage: crawl_arms.py <arm> <seed> <max_pages> <max_depth> <concurrency> <out.json> [path-prefix] [js]
Writes {"pages": [{"url", "ok", "chars", "markdown"}]} to out.json. Only URLs whose path starts with
path-prefix are followed. With `js`, the site builds its content in the browser: Scrapling crawls it
through its browser session, the setup its docs give for dynamic pages; webclaw renders on its own
and Crawl4AI always renders.
"""
import asyncio, json, os, subprocess, sys
from urllib.parse import urljoin, urlparse

ARM, SEED, MAX_PAGES, MAX_DEPTH, CONCURRENCY, OUT = sys.argv[1:7]
MAX_PAGES, MAX_DEPTH, CONCURRENCY = int(MAX_PAGES), int(MAX_DEPTH), int(CONCURRENCY)
PREFIX = sys.argv[7] if len(sys.argv) > 7 else "/"
JS = len(sys.argv) > 8 and sys.argv[8] == "js"


def webclaw():
    r = subprocess.run([os.environ["WEBCLAW_BIN"], SEED, "--crawl", "--depth", str(MAX_DEPTH),
                        "--max-pages", str(MAX_PAGES), "--concurrency", str(CONCURRENCY), "--delay", "0",
                        "--path-prefix", PREFIX, "-f", "json"], capture_output=True, text=True)
    pages = json.loads(r.stdout or '{"pages": []}')["pages"]
    out = []
    for p in pages:
        md = (p.get("extraction") or {}).get("content", {}).get("markdown", "")
        out.append({"url": p["url"], "ok": p.get("extraction") is not None, "chars": len(md), "markdown": md})
    return out


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

        def configure_sessions(self, manager):
            if JS:
                from scrapling.fetchers import AsyncDynamicSession
                manager.add("default", AsyncDynamicSession(headless=True, max_pages=CONCURRENCY))
            else:
                super().configure_sessions(manager)

        async def parse(self, response):
            md = response.markdown()
            yield {"url": response.url, "chars": len(md), "markdown": md}
            depth = self.depth.get(response.url, 0)
            if depth >= MAX_DEPTH:
                return
            for href in response.css("a::attr(href)").getall():
                url = urljoin(response.url, href).split("#")[0]
                parsed = urlparse(url)
                if (parsed.hostname != host or not parsed.path.startswith(PREFIX) or url in self.depth
                        or self.scheduled >= MAX_PAGES):
                    continue
                self.depth[url] = depth + 1
                self.scheduled += 1
                yield response.follow(url, callback=self.parse)

    result = Crawl().start()
    return [{"url": i["url"], "ok": True, "chars": i["chars"], "markdown": i["markdown"]} for i in result.items]


def crawl4ai():
    from crawl4ai import AsyncWebCrawler, CrawlerRunConfig, CacheMode
    from crawl4ai.deep_crawling import BFSDeepCrawlStrategy
    from crawl4ai.deep_crawling.filters import FilterChain, URLPatternFilter

    async def go():
        scope = FilterChain([URLPatternFilter(patterns=[f"*{PREFIX}*"])]) if PREFIX != "/" else FilterChain()
        cfg = CrawlerRunConfig(
            deep_crawl_strategy=BFSDeepCrawlStrategy(max_depth=MAX_DEPTH, max_pages=MAX_PAGES, include_external=False,
                                                     filter_chain=scope),
            cache_mode=CacheMode.BYPASS, check_robots_txt=True, semaphore_count=CONCURRENCY, verbose=False)
        async with AsyncWebCrawler() as crawler:
            results = await crawler.arun(SEED, config=cfg)
        out = []
        for r in results:
            md = r.markdown.raw_markdown if r.success and r.markdown else ""
            out.append({"url": r.url, "ok": bool(r.success), "chars": len(md), "markdown": md})
        return out

    return asyncio.run(go())


pages = {"webclaw": webclaw, "scrapling": scrapling, "crawl4ai": crawl4ai}[ARM]()
json.dump({"pages": pages}, open(OUT, "w"))
