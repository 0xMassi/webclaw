"""One tool's attempts at the Cloudflare-protected pages of cloudflare-cases.json.

usage: cloudflare_arms.py <arm> <outdir> <cases.json>
Prints one JSON line per page: {"url", "file", "ms", "error", "token", "reuse_file"}. Every page runs in its own
process with a fresh browser, as one command per page does, and is cut off after LIMIT seconds. `file` holds what
the tool returned. `token` is the Turnstile token webclaw read from the page with an `executeJavascript` step, for
embedded widgets; the other tools return the whole page, which holds it. For a challenge page, `reuse_file` holds
what the tool's own plain HTTP client got, in the same process, for the page's URL with `?again=1`, sending the
cookies and the User-Agent of the browser that solved it. webclaw does that inside one run of two URLs; it is
measured by cloudflare.py.
"""
import asyncio, json, os, signal, subprocess, sys, time
from pathlib import Path

TIMEOUT = 60
LIMIT = 150
TOKEN = ('(document.querySelector(\'[name="cf-turnstile-response"], [name="g-recaptcha-response"]\') '
         '|| {}).value || ""')


def again(url):
    return url + ("&" if "?" in url else "?") + "again=1"


def webclaw(url, extra):
    cmd = [os.environ["WEBCLAW_BIN"], url, "-f", "json", "--timeout", str(TIMEOUT)] + extra
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0 or not r.stdout.strip():
        raise RuntimeError(r.stderr.strip()[-200:] or f"exit {r.returncode}")
    results = json.loads(r.stdout).get("js_results") or [None]
    return r.stdout, results[0], None


def webclaw_solve(url, kind):
    if kind == "embedded":
        actions = json.dumps([{"type": "executeJavascript", "code": TOKEN}])
        return webclaw(url, ["--solve", "--render", "always", "--actions", actions])
    return webclaw(url, ["--solve"])


def scrapling(url, kind, real_chrome):
    from scrapling.fetchers import Fetcher, StealthyFetcher
    r = StealthyFetcher.fetch(url, headless=True, solve_cloudflare=True, real_chrome=real_chrome,
                              timeout=TIMEOUT * 1000)
    html = r.body.decode("utf-8", "replace") if isinstance(r.body, bytes) else str(r.body)
    reused = None
    if kind != "embedded":
        cookies = {c["name"]: c["value"] for c in r.cookies} if isinstance(r.cookies, tuple) else dict(r.cookies)
        ua = {k.lower(): v for k, v in (r.request_headers or {}).items()}.get("user-agent")
        try:
            got = Fetcher.get(again(url), cookies=cookies, headers={"User-Agent": ua}, impersonate="chrome",
                              stealthy_headers=False, timeout=30)
            reused = got.body.decode("utf-8", "replace") if isinstance(got.body, bytes) else str(got.body)
        except Exception as e:
            reused = f"error: {e}"
    return html, None, reused


async def crawl4ai(url):
    from crawl4ai import AsyncWebCrawler, BrowserConfig, CacheMode, CrawlerRunConfig
    from crawl4ai.async_crawler_strategy import AsyncPlaywrightCrawlerStrategy
    from crawl4ai.browser_adapter import UndetectedAdapter
    browser = BrowserConfig(headless=True, enable_stealth=True, verbose=False)
    run = CrawlerRunConfig(magic=True, simulate_user=True, override_navigator=True, cache_mode=CacheMode.BYPASS,
                           page_timeout=TIMEOUT * 1000, delay_before_return_html=5, verbose=False)
    strategy = AsyncPlaywrightCrawlerStrategy(browser_config=browser, browser_adapter=UndetectedAdapter())
    async with AsyncWebCrawler(crawler_strategy=strategy, config=browser) as crawler:
        r = await crawler.arun(url, config=run)
    if not r.html:
        raise RuntimeError((r.error_message or "no html")[-200:])
    return r.html, None, None


ARMS = {
    "webclaw-http": lambda u, k: webclaw(u, ["--render", "never"]),
    "webclaw-render": lambda u, k: webclaw(u, ["--render", "always"]),
    "webclaw-solve": webclaw_solve,
    "scrapling": lambda u, k: scrapling(u, k, False),
    "scrapling-real-chrome": lambda u, k: scrapling(u, k, True),
    "crawl4ai": lambda u, k: asyncio.run(crawl4ai(u)),
}


def one(arm, file, url, kind):
    try:
        html, token, reused = ARMS[arm](url, kind)
        Path(file).write_text(html)
        reuse_file = None
        if reused is not None:
            reuse_file = file + ".again"
            Path(reuse_file).write_text(reused)
        print(json.dumps({"token": token, "reuse_file": reuse_file}))
    except Exception as e:
        print(json.dumps({"error": str(e)[-200:]}))


def main(arm, out, cases):
    out.mkdir(parents=True, exist_ok=True)
    for i, case in enumerate(cases):
        url = case["url"]
        file = out / f"{i:02d}.html"
        file.unlink(missing_ok=True)
        t0 = time.perf_counter()
        p = subprocess.Popen([sys.executable, "-I", __file__, "--one", arm, str(file), url, case["kind"]],
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            stdout, stderr = p.communicate(timeout=LIMIT)
            lines = [l for l in stdout.splitlines() if l.startswith("{")]
            got = json.loads(lines[-1]) if lines else {"error": stderr.strip()[-200:] or f"exit {p.returncode}"}
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.communicate()
            got = {"error": f"no answer in {LIMIT}s"}
        ms = 1000 * (time.perf_counter() - t0)
        print(json.dumps({"url": url, "file": str(file) if file.exists() else None, "ms": ms,
                          "error": got.get("error"), "token": got.get("token"),
                          "reuse_file": got.get("reuse_file")}), flush=True)


if sys.argv[1] == "--one":
    one(*sys.argv[2:6])
else:
    main(sys.argv[1], Path(sys.argv[2]), json.loads(Path(sys.argv[3]).read_text()))
