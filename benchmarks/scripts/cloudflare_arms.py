"""One tool's attempts at the Cloudflare-protected pages of cloudflare-cases.json.

usage: cloudflare_arms.py <arm> <outdir> <cases.json>
Prints one JSON line per page: {"url", "file", "ms", "error", "cookies", "ua", "token"}. Every page runs in its
own process with a fresh browser, as one command per page does, and is cut off after LIMIT seconds. `file` holds
what the tool returned. `cookies` and `ua` are the browser's cookies and the User-Agent it sent, so a later plain
HTTP request can try to reuse them. `token` is the Turnstile token webclaw read from the page with an `eval`
action, for embedded widgets; the other tools return the whole page, which holds it.
"""
import asyncio, json, os, signal, subprocess, sys, time
from pathlib import Path

TIMEOUT = 60
LIMIT = 150
TOKEN = ('(document.querySelector(\'[name="cf-turnstile-response"], [name="g-recaptcha-response"]\') '
         '|| {}).value || ""')


def webclaw(url, extra):
    cmd = [os.environ["WEBCLAW_BIN"], url, "-f", "json", "--timeout", str(TIMEOUT)] + extra
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0 or not r.stdout.strip():
        raise RuntimeError(r.stderr.strip()[-200:] or f"exit {r.returncode}")
    token = next((json.loads(l[len("eval 1: "):]) for l in r.stderr.splitlines() if l.startswith("eval 1: ")), None)
    return r.stdout, {}, None, token


def webclaw_solve(url, kind):
    if kind == "embedded":
        actions = json.dumps([{"type": "eval", "script": TOKEN}])
        return webclaw(url, ["--solve", "--render", "always", "--actions", actions])
    return webclaw(url, ["--solve"])


def scrapling(url):
    from scrapling.fetchers import StealthyFetcher
    r = StealthyFetcher.fetch(url, headless=True, solve_cloudflare=True, timeout=TIMEOUT * 1000)
    html = r.body.decode("utf-8", "replace") if isinstance(r.body, bytes) else str(r.body)
    cookies = {c["name"]: c["value"] for c in r.cookies} if isinstance(r.cookies, tuple) else dict(r.cookies)
    ua = {k.lower(): v for k, v in (r.request_headers or {}).items()}.get("user-agent")
    return html, cookies, ua, None


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
    return r.html, {}, None, None


ARMS = {
    "webclaw-http": lambda u, k: webclaw(u, ["--render", "never"]),
    "webclaw-render": lambda u, k: webclaw(u, ["--render", "always"]),
    "webclaw-solve": webclaw_solve,
    "scrapling": lambda u, k: scrapling(u),
    "crawl4ai": lambda u, k: asyncio.run(crawl4ai(u)),
}


def one(arm, file, url, kind):
    try:
        html, cookies, ua, token = ARMS[arm](url, kind)
        Path(file).write_text(html)
        print(json.dumps({"cookies": cookies, "ua": ua, "token": token}))
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
                          "error": got.get("error"), "cookies": got.get("cookies") or {}, "ua": got.get("ua"),
                          "token": got.get("token")}), flush=True)


if sys.argv[1] == "--one":
    one(*sys.argv[2:6])
else:
    main(sys.argv[1], Path(sys.argv[2]), json.loads(Path(sys.argv[3]).read_text()))
