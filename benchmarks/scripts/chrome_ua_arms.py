"""One tool rendering the headless-check pages, a fresh browser per page, running the same probe script in each.

usage: chrome_ua_arms.py <arm> <rounds> <pages.json>
Prints one JSON line per page and round: {"page", "round", "ms", "error", "probe"}. `probe` is what
scripts/chrome_ua_probe.js returned in the page after waiting the page's `wait` milliseconds: the
User-Agent, navigator.webdriver, the client hints, a dedicated worker's User-Agent and the page text.
Arms: webclaw, webclaw-before, webclaw-before-solve (the CLI with --actions), webclaw-mcp (the MCP
scrape tool with actions), scrapling-dynamic, scrapling-stealthy, crawl4ai, crawl4ai-stealth.
"""
import asyncio, json, os, subprocess, sys, time
from pathlib import Path

ARM, ROUNDS, PAGES = sys.argv[1], int(sys.argv[2]), json.load(open(sys.argv[3]))
PROBE = (Path(__file__).parent / "chrome_ua_probe.js").read_text()


def probe(page):
    return PROBE.replace("WAIT_MS", str(page["wait"]))


def emit(page, rnd, t0, value, error=None):
    print(json.dumps({"page": page["name"], "round": rnd, "ms": 1000 * (time.perf_counter() - t0),
                      "error": error, "probe": value}), flush=True)


def webclaw(bin, extra):
    for rnd in range(ROUNDS):
        for page in PAGES:
            actions = json.dumps([{"type": "executeJavascript", "code": probe(page)}])
            t0 = time.perf_counter()
            r = subprocess.run([bin, page["url"], "--actions", actions, "-f", "json", "-t", "60", *extra],
                               capture_output=True, text=True, timeout=180)
            if r.returncode != 0:
                emit(page, rnd, t0, None, r.stderr.strip()[-300:])
                continue
            emit(page, rnd, t0, (json.loads(r.stdout).get("js_results") or [None])[0])


async def webclaw_mcp():
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    for rnd in range(ROUNDS):
        for page in PAGES:
            server = StdioServerParameters(command=os.environ["WEBCLAW_MCP_BIN"], env={**os.environ})
            t0 = time.perf_counter()
            try:
                async with stdio_client(server) as streams, ClientSession(*streams) as session:
                    await session.initialize()
                    r = await session.call_tool("scrape", {"url": page["url"], "format": "text", "actions": [
                        {"type": "executeJavascript", "code": probe(page)}]})
                text = "".join(getattr(c, "text", "") for c in r.content)
                if r.is_error:
                    emit(page, rnd, t0, None, text[-300:])
                    continue
                start = text.index("[", text.index("js_results"))
                emit(page, rnd, t0, json.JSONDecoder().raw_decode(text[start:])[0][0])
            except Exception as e:
                emit(page, rnd, t0, None, f"{type(e).__name__}: {e}"[-300:])


def scrapling(stealthy):
    from scrapling.fetchers import DynamicFetcher, StealthyFetcher
    fetcher = StealthyFetcher if stealthy else DynamicFetcher
    for rnd in range(ROUNDS):
        for page in PAGES:
            seen = {}
            t0 = time.perf_counter()
            try:
                fetcher.fetch(page["url"], headless=True, timeout=60000,
                              page_action=lambda p: seen.update(value=p.evaluate(probe(page))))
                emit(page, rnd, t0, seen.get("value"), None if "value" in seen else "the probe did not run")
            except Exception as e:
                emit(page, rnd, t0, None, f"{type(e).__name__}: {e}"[-300:])


def found(value):
    if isinstance(value, dict):
        if "ua" in value:
            return value
        value = list(value.values())
    if isinstance(value, list):
        return next((hit for hit in map(found, value) if hit), None)
    return None


async def crawl4ai(stealth):
    from crawl4ai import AsyncWebCrawler, BrowserConfig, CacheMode, CrawlerRunConfig
    browser = BrowserConfig(headless=True, verbose=False, enable_stealth=stealth)
    for rnd in range(ROUNDS):
        for page in PAGES:
            run = CrawlerRunConfig(cache_mode=CacheMode.BYPASS, page_timeout=60000, verbose=False,
                                   js_code=f"return await {probe(page)};")
            t0 = time.perf_counter()
            try:
                async with AsyncWebCrawler(config=browser) as crawler:
                    r = await crawler.arun(page["url"], config=run)
                value = found(r.js_execution_result)
                emit(page, rnd, t0, value, None if r.success and value else (r.error_message or "the probe did not run")[-300:])
            except Exception as e:
                emit(page, rnd, t0, None, f"{type(e).__name__}: {e}"[-300:])


if ARM == "webclaw":
    webclaw(os.environ["WEBCLAW_BIN"], [])
elif ARM == "webclaw-before":
    webclaw(os.environ["WEBCLAW_BASELINE_BIN"], [])
elif ARM == "webclaw-before-solve":
    webclaw(os.environ["WEBCLAW_BASELINE_BIN"], ["--solve"])
elif ARM == "webclaw-mcp":
    asyncio.run(webclaw_mcp())
elif ARM.startswith("scrapling"):
    scrapling(ARM == "scrapling-stealthy")
else:
    asyncio.run(crawl4ai(ARM == "crawl4ai-stealth"))
