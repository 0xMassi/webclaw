"""One resumable crawl by one tool, streaming pages to <dir>/out.jsonl as they finish.

usage: resume_arms.py <arm> <seed> <max_pages> <max_depth> <concurrency> <dir>
Each tool resumes through its own documented mechanism when <dir> holds a checkpoint.
"""
import asyncio, json, os, subprocess, sys
from pathlib import Path
from urllib.parse import urljoin, urlparse

ARM, SEED, MAX_PAGES, MAX_DEPTH, CONCURRENCY, DIR = sys.argv[1:7]
MAX_PAGES, MAX_DEPTH, CONCURRENCY = int(MAX_PAGES), int(MAX_DEPTH), int(CONCURRENCY)
DIR = Path(DIR)
OUT = DIR / "out.jsonl"


def emit(url, ok):
    with open(OUT, "a") as f:
        f.write(json.dumps({"url": url, "ok": ok}) + "\n")


def webclaw():
    proc = subprocess.Popen([os.environ["WEBCLAW_BIN"], SEED, "--crawl", "--depth", str(MAX_DEPTH),
                             "--max-pages", str(MAX_PAGES), "--concurrency", str(CONCURRENCY), "--delay", "0",
                             "-f", "jsonl", "--crawl-state", str(DIR / "state.json")],
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    for line in proc.stdout:
        page = json.loads(line)
        emit(page["url"], page.get("extraction") is not None)
    proc.wait()


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
            super().__init__(crawldir=DIR / "scrapling")
            self.scheduled = 1
            self.depth = {SEED: 0}

        async def parse(self, response):
            yield {"url": response.url}
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

    async def go():
        async for item in Crawl().stream():
            emit(item["url"], True)

    asyncio.run(go())


def crawl4ai():
    from crawl4ai import AsyncWebCrawler, CrawlerRunConfig, CacheMode
    from crawl4ai.deep_crawling import BFSDeepCrawlStrategy
    state_file = DIR / "crawl4ai-state.json"

    async def save(state):
        state_file.write_text(json.dumps(state))

    async def go():
        resume = json.loads(state_file.read_text()) if state_file.exists() else None
        cfg = CrawlerRunConfig(
            deep_crawl_strategy=BFSDeepCrawlStrategy(max_depth=MAX_DEPTH, max_pages=MAX_PAGES, include_external=False,
                                                     resume_state=resume, on_state_change=save),
            cache_mode=CacheMode.BYPASS, check_robots_txt=True, semaphore_count=CONCURRENCY, stream=True, verbose=False)
        async with AsyncWebCrawler() as crawler:
            async for r in await crawler.arun(SEED, config=cfg):
                emit(r.url, bool(r.success))

    asyncio.run(go())


DIR.mkdir(parents=True, exist_ok=True)
{"webclaw": webclaw, "scrapling": scrapling, "crawl4ai": crawl4ai}[ARM]()
