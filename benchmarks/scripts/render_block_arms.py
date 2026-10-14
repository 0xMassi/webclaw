"""One tool rendering a list of pages in one browser, with or without its resource blocking.

usage: render_block_arms.py <arm> <setting> <url>...
Every request goes through a local counting proxy, whose connections are closed between pages. Prints one JSON line per page:
{"url", "ms", "kb", "text", "error"}. The first page is a warm-up that starts the browser and is not
printed. webclaw runs through its MCP server's `scrape` tool with render "always" and `block` set to
<setting>; Scrapling through an AsyncDynamicSession (network_idle) with disable_resources on or off;
Crawl4AI through one AsyncWebCrawler with text_mode on or off.
"""
import asyncio, json, os, sys, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import count_proxy

ARM, SETTING, URLS = sys.argv[1], sys.argv[2], sys.argv[3:]
WARMUP = "https://quotes.toscrape.com/js/page/10/"
PROXY, COUNTER, CUT = count_proxy.start()


def emit(url, t0, kb0, text, error=None):
    print(json.dumps({"url": url, "ms": 1000 * (time.perf_counter() - t0),
                      "kb": (COUNTER["down"] - kb0) / 1024, "text": text or "", "error": error}), flush=True)


async def each(fetch):
    await fetch(WARMUP)
    for url in URLS:
        CUT()
        await asyncio.sleep(1)
        t0, kb0 = time.perf_counter(), COUNTER["down"]
        try:
            text, error = await fetch(url), None
        except Exception as e:
            text, error = "", f"{type(e).__name__}: {e}"[-300:]
        emit(url, t0, kb0, text, error)


async def webclaw():
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    env = {**os.environ, "WEBCLAW_PROXY": PROXY}
    env.pop("WEBCLAW_API_KEY", None)
    server = StdioServerParameters(command=os.environ["WEBCLAW_MCP_BIN"], env=env)
    async with stdio_client(server) as streams, ClientSession(*streams) as session:
        await session.initialize()

        async def fetch(url):
            r = await session.call_tool("scrape", {"url": url, "format": "markdown", "render": "always",
                                                   "block": SETTING})
            text = "".join(getattr(c, "text", "") for c in r.content)
            if r.is_error:
                raise RuntimeError(text[-300:])
            return text
        await each(fetch)


async def scrapling():
    from scrapling.fetchers import AsyncDynamicSession
    async with AsyncDynamicSession(headless=True, network_idle=True, disable_resources=SETTING == "on",
                                   proxy=PROXY) as session:
        async def fetch(url):
            return (await session.fetch(url)).markdown()
        await each(fetch)


async def crawl4ai():
    from crawl4ai import AsyncWebCrawler, BrowserConfig, CacheMode, CrawlerRunConfig
    browser = BrowserConfig(headless=True, verbose=False, text_mode=SETTING == "on",
                            proxy_config={"server": PROXY})
    async with AsyncWebCrawler(config=browser) as crawler:
        async def fetch(url):
            r = await crawler.arun(url, config=CrawlerRunConfig(cache_mode=CacheMode.BYPASS, verbose=False,
                                                                page_timeout=30000))
            if not r.success:
                raise RuntimeError((r.error_message or "crawl failed")[-300:])
            return r.markdown.raw_markdown
        await each(fetch)


asyncio.run({"webclaw": webclaw, "scrapling": scrapling, "crawl4ai": crawl4ai}[ARM]())
