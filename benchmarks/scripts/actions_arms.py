"""One tool's runs of the page-action cases.

usage: actions_arms.py <arm> <outdir> <cases.json> [case-name ...]
Prints one JSON line per case: {"case", "file", "ms", "error"}, where `file` holds the HTML the tool
returned. Every case starts a fresh browser, as one command per page does.

Arms:
  webclaw       the CLI with --actions (the case's steps as they are)
  webclaw-none  the CLI with --render always and no actions: the number before this feature
  scrapling     DynamicFetcher with a page_action that runs the steps through Playwright, since it has
                no scroll or click helpers of its own: a fixed 1 s pause after each click, key press and
                scroll to the end, 0.3 s after each scroll by pixels. A "load more" loop waits up to 10 s
                for a visible button the first time and 2 s after, clicks it with force=True (a cookie
                banner covers NPR's) and pauses 1.5 s
  crawl4ai      its own options where it has them: scan_full_page with its documented scroll_delay=1.0
                for scrolling, wait_for for waiting, C4A-Script for typing, key presses and single
                clicks, and a js_code loop for a "load more" button, since C4A-Script's REPEAT takes a
                count fixed before the loop starts and cannot stop once the button is gone. The loop
                waits up to 10 s for a visible button the first time and 2 s after, clicks it and
                pauses 1.5 s
"""
import asyncio, json, os, subprocess, sys, time
from pathlib import Path

ARM, OUT, CASES = sys.argv[1], Path(sys.argv[2]), json.loads(Path(sys.argv[3]).read_text())
ONLY = set(sys.argv[4:])
OUT.mkdir(parents=True, exist_ok=True)
TIMEOUT_S = 90


def emit(case, html, ms, error=None):
    file = None
    if html:
        file = OUT / f"{case['name']}.html"
        file.write_text(html)
    print(json.dumps({"case": case["name"], "file": str(file) if file else None, "ms": ms, "error": error}),
          flush=True)


def webclaw(case, actions):
    cmd = [os.environ["WEBCLAW_BIN"], case["url"], "-f", "html", "--timeout", "60"]
    cmd += ["--actions", json.dumps(case["steps"])] if actions else ["--render", "always"]
    t0 = time.perf_counter()
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=TIMEOUT_S)
        ok = r.returncode == 0 and r.stdout.strip()
        emit(case, r.stdout if ok else None, 1000 * (time.perf_counter() - t0),
             None if ok else r.stderr.strip()[-300:])
    except subprocess.TimeoutExpired:
        emit(case, None, 1000 * (time.perf_counter() - t0), f"no answer in {TIMEOUT_S}s")


def playwright_steps(steps):
    def act(page):
        for s in steps:
            kind = s["type"]
            if kind == "click":
                loc = page.locator(s["selector"])
                if not s.get("repeat"):
                    loc.nth(s.get("nth") or 0).click(timeout=10000)
                    page.wait_for_timeout(1000)
                    continue
                shown = page.locator(f"{s['selector']} >> visible=true").first
                for i in range(s["repeat"]):
                    try:
                        shown.wait_for(timeout=10000 if i == 0 else 2000)
                    except Exception:
                        break
                    shown.click(timeout=10000, force=True)
                    page.wait_for_timeout(1500)
            elif kind == "type":
                page.locator(s["selector"]).first.press_sequentially(s["value"])
            elif kind == "press":
                page.keyboard.press(s["key"])
                page.wait_for_timeout(1000)
            elif kind == "scroll":
                for _ in range(s.get("repeat") or 1):
                    height = page.evaluate("document.documentElement.scrollHeight")
                    if s.get("amount"):
                        page.mouse.wheel(0, s["amount"] if s.get("direction", "down") == "down" else -s["amount"])
                        page.wait_for_timeout(300)
                    else:
                        page.evaluate("window.scrollTo(0, document.documentElement.scrollHeight)")
                        page.wait_for_timeout(1000)
                        if page.evaluate("document.documentElement.scrollHeight") == height:
                            break
            elif kind == "wait":
                page.wait_for_timeout(s["milliseconds"])
            elif kind == "waitForSelector":
                page.wait_for_selector(s["selector"], timeout=s.get("timeout") or 10000)
            elif kind == "executeJavascript":
                page.evaluate(s["code"])
        return page
    return act


def scrapling(case):
    from scrapling.fetchers import DynamicFetcher
    t0 = time.perf_counter()
    try:
        r = DynamicFetcher.fetch(case["url"], page_action=playwright_steps(case["steps"]), headless=True,
                                 network_idle=True, timeout=60000)
        html = r.html_content if isinstance(r.html_content, str) else r.body.decode("utf-8", "replace")
        emit(case, html, 1000 * (time.perf_counter() - t0))
    except Exception as e:
        emit(case, None, 1000 * (time.perf_counter() - t0), str(e)[-300:])


def c4a(steps):
    lines = []
    for s in steps:
        if s["type"] == "click":
            lines.append(f"CLICK `{s['selector']}`")
            lines.append("WAIT 1")
        elif s["type"] == "type":
            lines += [f"CLICK `{s['selector']}`", f"TYPE {json.dumps(s['value'])}"]
        elif s["type"] == "press":
            lines += [f"PRESS {s['key']}", "WAIT 1"]
        elif s["type"] == "wait":
            lines.append(f"WAIT {s['milliseconds'] / 1000}")
    return "\n".join(lines)


def crawl4ai_config(steps):
    from crawl4ai import CacheMode, CrawlerRunConfig
    opts = {"cache_mode": CacheMode.BYPASS, "page_timeout": 60000, "verbose": False}
    kinds = {s["type"] for s in steps}
    if kinds == {"scroll"}:
        opts.update(scan_full_page=True, scroll_delay=1.0, max_scroll_steps=600)
    elif kinds == {"waitForSelector"}:
        s = steps[0]
        opts.update(wait_for=f"css:{s['selector']}", wait_for_timeout=s.get("timeout") or 10000)
    elif kinds == {"click"} and steps[0].get("repeat"):
        s = steps[0]
        opts["js_code"] = (
            f"for (let i = 0; i < {s['repeat']}; i++) {{ let b; const t = Date.now(); const wait = i ? 2000 : 10000; "
            f"while (!(b = [...document.querySelectorAll({json.dumps(s['selector'])})].find(e => e.offsetParent)) && Date.now() - t < wait) "
            "await new Promise(r => setTimeout(r, 200)); "
            "if (!b) break; b.click(); await new Promise(r => setTimeout(r, 1500)); }")
    else:
        opts["c4a_script"] = c4a(steps)
    return CrawlerRunConfig(**opts)


async def crawl4ai_all(cases):
    from crawl4ai import AsyncWebCrawler, BrowserConfig
    for case in cases:
        t0 = time.perf_counter()
        try:
            async with AsyncWebCrawler(config=BrowserConfig(headless=True, verbose=False)) as crawler:
                r = await asyncio.wait_for(crawler.arun(case["url"], config=crawl4ai_config(case["steps"])),
                                           TIMEOUT_S)
            if not r.success:
                raise RuntimeError(r.error_message or "failed")
            emit(case, r.html, 1000 * (time.perf_counter() - t0))
        except Exception as e:
            emit(case, None, 1000 * (time.perf_counter() - t0), str(e)[-300:])


cases = [c for c in CASES if not ONLY or c["name"] in ONLY]
if ARM == "crawl4ai":
    asyncio.run(crawl4ai_all(cases))
else:
    for case in cases:
        if ARM == "scrapling":
            scrapling(case)
        else:
            webclaw(case, ARM == "webclaw")
