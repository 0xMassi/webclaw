"""One tool, called through its Python API, inside its own fresh venv.

usage: python_arms.py <arm> first <url>
       python_arms.py <arm> steady <urls.json> <rounds>
first: import the package and scrape one page, print {"import_s", "call_s", "ok", "text", "error"}.
steady: import, one warm-up call, then every URL per round, one JSON line per call.
"""
import asyncio, atexit, json, sys, time

T0 = time.perf_counter()
ARM, MODE = sys.argv[1:3]
TIMEOUT = 30


def webclaw():
    import webclaw_cli
    return lambda url: webclaw_cli.scrape(url, "llm", timeout=TIMEOUT)


def scrapling():
    from scrapling.fetchers import Fetcher
    return lambda url: Fetcher.get(url, timeout=TIMEOUT).markdown()


def crawl4ai():
    from crawl4ai import AsyncWebCrawler, CacheMode, CrawlerRunConfig
    loop = asyncio.new_event_loop()
    crawler = AsyncWebCrawler()
    loop.run_until_complete(crawler.start())
    atexit.register(lambda: loop.run_until_complete(crawler.close()))
    config = CrawlerRunConfig(cache_mode=CacheMode.BYPASS, page_timeout=TIMEOUT * 1000)

    def call(url):
        res = loop.run_until_complete(asyncio.wait_for(crawler.arun(url, config=config), TIMEOUT + 15))
        if not res.success:
            raise RuntimeError(res.error_message or "crawl failed")
        return res.markdown.raw_markdown

    return call


def timed(fn, url):
    t = time.perf_counter()
    text, error = "", None
    try:
        text = fn(url) or ""
    except Exception as e:
        error = f"{type(e).__name__}: {e}"[:300]
    return {"url": url, "call_s": round(time.perf_counter() - t, 3), "ok": bool(text.strip()) and error is None,
            "text": text, "error": error}


def main():
    fn = {"webclaw": webclaw, "scrapling": scrapling, "crawl4ai": crawl4ai}[ARM]()
    import_s = round(time.perf_counter() - T0, 3)
    if MODE == "first":
        print(json.dumps({"import_s": import_s, **timed(fn, sys.argv[3])}), flush=True)
        return
    urls = json.load(open(sys.argv[3]))
    timed(fn, urls[0])
    for rnd in range(int(sys.argv[4])):
        for url in urls:
            print(json.dumps({"round": rnd, **timed(fn, url)}), flush=True)


main()
