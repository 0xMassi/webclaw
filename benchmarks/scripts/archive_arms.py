"""One tool's MHTML archives of a list of pages.

usage: archive_arms.py <arm> <cold|warm> <outdir> <url>...
Prints one JSON line per page: {"url", "file", "ms", "error"}. `cold` starts a fresh browser for
every page, as one command per archive does. `warm` keeps one browser for all pages: webclaw through
its MCP server's `screenshot` tool with format "mhtml", Crawl4AI with one AsyncWebCrawler.
"""
import asyncio, base64, json, os, subprocess, sys, time
from pathlib import Path

ARM, MODE, OUT, URLS = sys.argv[1], sys.argv[2], Path(sys.argv[3]), sys.argv[4:]
OUT.mkdir(parents=True, exist_ok=True)


def emit(i, url, data, ms, error=None):
    file = None
    if data:
        file = OUT / f"{i:02d}.mhtml"
        file.write_bytes(data)
    print(json.dumps({"url": url, "file": str(file) if file else None, "ms": ms, "error": error}), flush=True)


def webclaw_cold():
    for i, url in enumerate(URLS):
        file = OUT / f"{i:02d}.out.mhtml"
        cmd = [os.environ["WEBCLAW_BIN"], url, "--save-mhtml", str(file)]
        t0 = time.perf_counter()
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
        ms = 1000 * (time.perf_counter() - t0)
        data = file.read_bytes() if r.returncode == 0 and file.exists() else None
        file.unlink(missing_ok=True)
        emit(i, url, data, ms, None if data else r.stderr.strip()[-200:])


async def webclaw_warm():
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    server = StdioServerParameters(command=os.environ["WEBCLAW_MCP_BIN"], env={**os.environ})
    async with stdio_client(server) as streams, ClientSession(*streams) as session:
        await session.initialize()
        for i, url in enumerate(URLS):
            t0 = time.perf_counter()
            r = await session.call_tool("screenshot", {"url": url, "format": "mhtml"})
            ms = 1000 * (time.perf_counter() - t0)
            blobs = [c.resource for c in r.content if getattr(c, "type", "") == "resource"
                     and getattr(c.resource, "mime_type", "") == "multipart/related"]
            if r.is_error or not blobs:
                emit(i, url, None, ms, "".join(getattr(c, "text", "") for c in r.content)[-200:])
            else:
                emit(i, url, base64.b64decode(blobs[0].blob), ms)


async def crawl4ai_run(crawler, url):
    from crawl4ai import CacheMode, CrawlerRunConfig
    r = await crawler.arun(url, config=CrawlerRunConfig(capture_mhtml=True, cache_mode=CacheMode.BYPASS,
                                                        verbose=False))
    if not r.success or not r.mhtml:
        raise RuntimeError((r.error_message or "no mhtml")[-200:])
    return r.mhtml.encode()


async def timed(i, url, coro):
    t0 = time.perf_counter()
    try:
        data, error = await coro, None
    except Exception as e:
        data, error = None, str(e)[-200:]
    emit(i, url, data, 1000 * (time.perf_counter() - t0), error)


async def crawl4ai_all(warm):
    from crawl4ai import AsyncWebCrawler, BrowserConfig
    browser = BrowserConfig(headless=True, verbose=False)
    if warm:
        async with AsyncWebCrawler(config=browser) as crawler:
            for i, url in enumerate(URLS):
                await timed(i, url, crawl4ai_run(crawler, url))
    else:
        for i, url in enumerate(URLS):
            async def one():
                async with AsyncWebCrawler(config=browser) as crawler:
                    return await crawl4ai_run(crawler, url)
            await timed(i, url, one())


if ARM == "crawl4ai":
    asyncio.run(crawl4ai_all(MODE == "warm"))
elif MODE == "warm":
    asyncio.run(webclaw_warm())
else:
    webclaw_cold()
