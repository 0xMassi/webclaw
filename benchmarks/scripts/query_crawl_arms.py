"""One question-guided crawl by one tool. Runs inside that tool's venv.

usage: query_crawl_arms.py <arm> <seed> <path-prefix> <budget> <depth> <concurrency> <out.json> [question]
Writes {"pages": [{"url", "t"}], "ranked": [url, ...], "previews": n}: pages in the order the tool fetched
them, `t` seconds after start, and the tool's own best-first ranking of its output. Breadth-first arms take
no question.
"""
import asyncio, json, os, re, subprocess, sys, time
from urllib.parse import urljoin, urlparse

ARM, SEED, PREFIX, BUDGET, DEPTH, CONCURRENCY, OUT = sys.argv[1:8]
BUDGET, DEPTH, CONCURRENCY = int(BUDGET), int(DEPTH), int(CONCURRENCY)
QUESTION = sys.argv[8] if len(sys.argv) > 8 else None
HOST = urlparse(SEED).hostname
STOP = {"the", "and", "for", "with", "what", "how", "does", "can", "which", "why", "when", "who", "from", "into",
        "that", "this", "about", "are", "you", "your", "its", "not", "use", "using", "there", "all", "any"}
START = time.time()


def in_scope(url):
    u = urlparse(url)
    return u.hostname == HOST and u.path.startswith(PREFIX)


def keywords(question):
    return [w for w in re.findall(r"\w+", question.lower()) if len(w) > 2 and w not in STOP]


def webclaw():
    cmd = [os.environ["WEBCLAW_BIN"], SEED, "--crawl", "--depth", str(DEPTH), "--max-pages", str(BUDGET),
           "--concurrency", str(CONCURRENCY), "--delay", "0", "--path-prefix", PREFIX, "-f", "jsonl"]
    if QUESTION:
        cmd += ["--query", QUESTION]
    if ARM == "webclaw-query-stop":
        cmd += ["--stop-when-answered"]
    if os.environ.get("QC_CACHE"):
        cmd += ["--cache"]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    pages = []
    for line in proc.stdout:
        p = json.loads(line)
        pages.append({"url": p["url"], "t": time.time() - START, "relevance": p.get("relevance", 0)})
    proc.wait()
    ranked = sorted(pages, key=lambda p: -p["relevance"]) if QUESTION else pages
    return pages, [p["url"] for p in ranked], 0


def scrapling():
    from scrapling.spiders import Spider
    pages = []

    class Crawl(Spider):
        name = "bench"
        start_urls = [SEED]
        allowed_domains = {HOST}
        robots_txt_obey = True
        concurrent_requests = CONCURRENCY
        logging_level = 40

        def __init__(self):
            super().__init__()
            self.depth = {SEED: 0}

        async def parse(self, response):
            pages.append({"url": response.url, "t": time.time() - START})
            yield {"url": response.url}
            depth = self.depth.get(response.request.url, 0)
            if depth >= DEPTH:
                return
            for href in response.css("a::attr(href)").getall():
                url = urljoin(response.url, href).split("#")[0]
                if not in_scope(url) or url in self.depth or len(self.depth) >= BUDGET:
                    continue
                self.depth[url] = depth + 1
                yield response.follow(url, callback=self.parse)

    Crawl().start()
    return pages, [p["url"] for p in pages], 0


async def crawl4ai_deep(best_first):
    from crawl4ai import AsyncWebCrawler, CacheMode, CrawlerRunConfig
    from crawl4ai.deep_crawling import BestFirstCrawlingStrategy, BFSDeepCrawlStrategy
    from crawl4ai.deep_crawling.filters import FilterChain, URLPatternFilter
    from crawl4ai.deep_crawling.scorers import KeywordRelevanceScorer

    scope = FilterChain([URLPatternFilter(patterns=[f"*{HOST}{PREFIX}*"])])
    if best_first:
        strategy = BestFirstCrawlingStrategy(max_depth=DEPTH, max_pages=BUDGET, filter_chain=scope,
                                             url_scorer=KeywordRelevanceScorer(keywords=keywords(QUESTION)))
    else:
        strategy = BFSDeepCrawlStrategy(max_depth=DEPTH, max_pages=BUDGET, filter_chain=scope)
    cfg = CrawlerRunConfig(deep_crawl_strategy=strategy, cache_mode=CacheMode.BYPASS, check_robots_txt=True,
                           semaphore_count=CONCURRENCY, stream=True, verbose=False)
    pages = []
    async with AsyncWebCrawler() as crawler:
        async for r in await crawler.arun(SEED, config=cfg):
            if r.success:
                pages.append({"url": r.url, "t": time.time() - START, "score": (r.metadata or {}).get("score", 0)})
    ranked = sorted(pages, key=lambda p: -p["score"]) if best_first else pages
    return pages, [p["url"] for p in ranked], 0


async def crawl4ai_adaptive(docs):
    from crawl4ai import AdaptiveConfig, AdaptiveCrawler, AsyncWebCrawler, CacheMode
    from crawl4ai.async_configs import CrawlerRunConfig, LinkPreviewConfig

    previews = 0
    pages = []

    async def scoped_preview(self, url, query):
        nonlocal previews
        cfg = CrawlerRunConfig(
            link_preview_config=LinkPreviewConfig(include_internal=True, include_external=False,
                                                  include_patterns=[f"*{HOST}{PREFIX}*"], query=query,
                                                  concurrency=5, timeout=self.config.link_preview_timeout,
                                                  max_links=50, verbose=False),
            score_links=True, check_robots_txt=True,
            cache_mode=CacheMode.ENABLED if os.environ.get("QC_CACHE") else CacheMode.BYPASS)
        try:
            result = await self.crawler.arun(url=url, config=cfg)
            if hasattr(result, "_results") and result._results:
                result = result._results[0]
            if result.links:
                result.links["internal"] = [l for l in result.links["internal"]
                                            if l.get("head_data") and in_scope(l["href"])]
                previews += len(result.links["internal"])
            if result.success:
                pages.append({"url": url, "t": time.time() - START})
            return result
        except Exception:
            return None

    AdaptiveCrawler._crawl_with_preview = scoped_preview
    tuned = dict(confidence_threshold=0.8, top_k_links=5, min_gain_threshold=0.05) if docs else {}
    config = AdaptiveConfig(max_pages=BUDGET, max_depth=BUDGET, strategy="statistical", **tuned)
    async with AsyncWebCrawler() as crawler:
        adaptive = AdaptiveCrawler(crawler, config)
        await adaptive.digest(start_url=SEED, query=QUESTION)
        ranked = [d["url"] for d in adaptive.get_relevant_content(top_k=len(pages))]
    return pages, ranked, previews


ARMS = {
    "webclaw-bfs": webclaw,
    "webclaw-query": webclaw,
    "webclaw-query-stop": webclaw,
    "scrapling-bfs": scrapling,
    "crawl4ai-bfs": lambda: asyncio.run(crawl4ai_deep(False)),
    "crawl4ai-best-first": lambda: asyncio.run(crawl4ai_deep(True)),
    "crawl4ai-adaptive": lambda: asyncio.run(crawl4ai_adaptive(False)),
    "crawl4ai-adaptive-docs": lambda: asyncio.run(crawl4ai_adaptive(True)),
}
pages, ranked, previews = ARMS[ARM]()
json.dump({"pages": pages, "ranked": ranked, "previews": previews, "seconds": time.time() - START}, open(OUT, "w"))
