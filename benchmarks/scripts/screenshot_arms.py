"""One tool's screenshots of a list of pages.

usage: screenshot_arms.py <arm> <cold|warm> <full:0|1> <outdir> <url>...
Prints one JSON line per page: {"url", "file", "ms", "error"}. `cold` starts a fresh browser for
every page, as one command per screenshot does. `warm` keeps one browser for all pages: webclaw and
Scrapling through their MCP servers (Scrapling with an open browser session), Crawl4AI with one
AsyncWebCrawler.
"""
import asyncio, base64, json, os, subprocess, sys, time
from pathlib import Path

ARM, MODE, FULL, OUT, URLS = sys.argv[1], sys.argv[2], sys.argv[3] == "1", Path(sys.argv[4]), sys.argv[5:]
OUT.mkdir(parents=True, exist_ok=True)


def emit(i, url, data, ms, error=None):
    file = None
    if data:
        file = OUT / f"{i:02d}.img"
        file.write_bytes(data)
    print(json.dumps({"url": url, "file": str(file) if file else None, "ms": ms, "error": error}), flush=True)


def webclaw_cold():
    for i, url in enumerate(URLS):
        file = OUT / f"{i:02d}.png"
        cmd = [os.environ["WEBCLAW_BIN"], url, "--screenshot", str(file)] + (["--full-page"] if FULL else [])
        t0 = time.perf_counter()
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
        ms = 1000 * (time.perf_counter() - t0)
        data = file.read_bytes() if r.returncode == 0 and file.exists() else None
        file.unlink(missing_ok=True)
        emit(i, url, data, ms, None if data else r.stderr.strip()[-200:])


def scrapling_cold():
    from scrapling.fetchers import DynamicFetcher
    for i, url in enumerate(URLS):
        shot = {}

        def capture(page):
            shot["png"] = page.screenshot(full_page=FULL, type="png")
            return page

        t0 = time.perf_counter()
        try:
            DynamicFetcher.fetch(url, page_action=capture, headless=True)
            error = None
        except Exception as e:
            error = str(e)[-200:]
        emit(i, url, shot.get("png"), 1000 * (time.perf_counter() - t0), error)


async def crawl4ai_run(crawler, url):
    from crawl4ai import CacheMode, CrawlerRunConfig
    cfg = CrawlerRunConfig(screenshot=True, scan_full_page=FULL, force_viewport_screenshot=not FULL,
                           cache_mode=CacheMode.BYPASS,
                           verbose=False)
    r = await crawler.arun(url, config=cfg)
    if not r.success or not r.screenshot:
        raise RuntimeError((r.error_message or "no screenshot")[-200:])
    return base64.b64decode(r.screenshot)


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


async def timed(i, url, coro):
    t0 = time.perf_counter()
    try:
        data, error = await coro, None
    except Exception as e:
        data, error = None, str(e)[-200:]
    emit(i, url, data, 1000 * (time.perf_counter() - t0), error)


async def mcp_warm(arm):
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    if arm == "webclaw":
        server = StdioServerParameters(command=os.environ["WEBCLAW_MCP_BIN"], env={**os.environ})
    else:
        server = StdioServerParameters(command=sys.executable, env={**os.environ},
                                       args=["-c", "import sys; from scrapling.cli import main; sys.exit(main())", "mcp"])
    async with stdio_client(server) as streams, ClientSession(*streams) as session:
        await session.initialize()
        if arm == "webclaw":
            args = lambda u: {"url": u, "full_page": FULL}
        else:
            opened = await session.call_tool("open_session", {"session_type": "dynamic"})
            sid = json.loads(opened.content[0].text)["session_id"]
            args = lambda u: {"url": u, "session_id": sid, "full_page": FULL}
        for i, url in enumerate(URLS):
            t0 = time.perf_counter()
            r = await session.call_tool("screenshot", args(url))
            ms = 1000 * (time.perf_counter() - t0)
            images = [c for c in r.content if getattr(c, "type", "") == "image"]
            if r.is_error or not images:
                emit(i, url, None, ms, "".join(getattr(c, "text", "") for c in r.content)[-200:])
            else:
                emit(i, url, base64.b64decode(images[0].data), ms)


if ARM == "crawl4ai":
    asyncio.run(crawl4ai_all(MODE == "warm"))
elif MODE == "warm":
    asyncio.run(mcp_warm(ARM))
elif ARM == "webclaw":
    webclaw_cold()
else:
    scrapling_cold()
