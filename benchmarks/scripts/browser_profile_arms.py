"""One tool loading one page on a persistent browser profile, in a process of its own.

usage: browser_profile_arms.py <arm> <profile-dir|-> <url> [steps.json|-] [hold-seconds]
Prints one JSON line: {"text", "values", "ms", "error"}. `text` is what the tool returned for the
page (webclaw's JSON output, the other tools' HTML), `values` the results of the executeJavascript
steps. `-` as the profile runs without one. With hold-seconds, the browser stays open on the profile
that long after the page loaded, so another process can try the same profile meanwhile.

Arms, each with its documented persistent-profile option:
  webclaw    the CLI with --browser-profile, and --actions for the steps; a script body runs as
             an async function expression, since --actions takes a body with await and return
             only in that form
  scrapling  DynamicFetcher with user_data_dir, the steps through a Playwright page_action
  crawl4ai   BrowserConfig(use_persistent_context=True, user_data_dir=...), C4A-Script for typing
             and clicking, js_code for scripts
"""
import asyncio, json, os, subprocess, sys, time

ARM, PROFILE, URL = sys.argv[1], sys.argv[2], sys.argv[3]
STEPS = json.loads(open(sys.argv[4]).read()) if len(sys.argv) > 4 and sys.argv[4] != "-" else []
HOLD = float(sys.argv[5]) if len(sys.argv) > 5 else 0
PROFILE = None if PROFILE == "-" else PROFILE
TIMEOUT_S = 90


def emit(t0, text=None, values=None, error=None):
    print(json.dumps({"text": text, "values": values or [], "ms": 1000 * (time.perf_counter() - t0),
                      "error": error}), flush=True)


def webclaw():
    cmd = [os.environ["WEBCLAW_BIN"], URL, "-f", "json", "--timeout", "45"]
    if PROFILE:
        cmd += ["--browser-profile", PROFILE]
    else:
        cmd += ["--render", "always"]
    steps = [{**s, "code": f"(async () => {{ {s['code']} }})()"} if s["type"] == "executeJavascript" else s
             for s in STEPS]
    steps += [{"type": "wait", "milliseconds": int(HOLD * 1000)}] if HOLD else []
    if steps:
        cmd += ["--actions", json.dumps(steps)]
    t0 = time.perf_counter()
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=TIMEOUT_S)
    except subprocess.TimeoutExpired:
        return emit(t0, error=f"no answer in {TIMEOUT_S}s")
    if r.returncode != 0:
        return emit(t0, error=r.stderr.strip()[-300:])
    values = json.loads(r.stdout).get("js_results", [])
    emit(t0, r.stdout, values)


def playwright_steps(steps, values):
    def act(page):
        for s in steps:
            if s["type"] == "type":
                page.locator(s["selector"]).first.press_sequentially(s["value"])
            elif s["type"] == "click":
                page.locator(s["selector"]).first.click(timeout=10000)
                page.wait_for_load_state("networkidle")
            elif s["type"] == "executeJavascript":
                values.append(page.evaluate(f"async () => {{ {s['code']} }}"))
        if HOLD:
            page.wait_for_timeout(HOLD * 1000)
        return page
    return act


def scrapling():
    from scrapling.fetchers import DynamicFetcher
    values = []
    kwargs = {"headless": True, "network_idle": True, "timeout": 45000,
              "page_action": playwright_steps(STEPS, values)}
    if PROFILE:
        kwargs["user_data_dir"] = PROFILE
    t0 = time.perf_counter()
    try:
        r = DynamicFetcher.fetch(URL, **kwargs)
        html = r.html_content if isinstance(r.html_content, str) else r.body.decode("utf-8", "replace")
        emit(t0, html, values)
    except Exception as e:
        emit(t0, error=str(e)[-300:])


async def crawl4ai():
    from crawl4ai import AsyncWebCrawler, BrowserConfig, CacheMode, CrawlerRunConfig
    browser = {"headless": True, "verbose": False}
    if PROFILE:
        browser.update(use_persistent_context=True, user_data_dir=PROFILE)
    run = {"cache_mode": CacheMode.BYPASS, "page_timeout": 45000, "verbose": False}
    script = []
    for s in STEPS:
        if s["type"] == "type":
            script += [f"CLICK `{s['selector']}`", f"TYPE {json.dumps(s['value'])}"]
        elif s["type"] == "click":
            script += [f"CLICK `{s['selector']}`", "WAIT 2"]
    if script:
        run["c4a_script"] = "\n".join(script)
    codes = [s["code"] for s in STEPS if s["type"] == "executeJavascript"]
    if codes:
        run["js_code"] = codes
    if HOLD:
        run["delay_before_return_html"] = HOLD
    t0 = time.perf_counter()
    try:
        async with AsyncWebCrawler(config=BrowserConfig(**browser)) as crawler:
            r = await asyncio.wait_for(crawler.arun(URL, config=CrawlerRunConfig(**run)), TIMEOUT_S)
        if not r.success:
            raise RuntimeError(r.error_message or "failed")
        results = (r.js_execution_result or {}).get("results", [])
        emit(t0, r.html, results)
    except Exception as e:
        emit(t0, error=str(e)[-300:])


if ARM == "crawl4ai":
    asyncio.run(crawl4ai())
elif ARM == "scrapling":
    scrapling()
else:
    webclaw()
