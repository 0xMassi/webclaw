"""One benchmark arm over a case list. Runs inside that arm's own venv.

usage: arms.py <arm> <url|html> <cases.jsonl> <out.jsonl> <outdir> [runs]
Each output row: arm, mode, id, run, ok, seconds, error. Text goes to <outdir>/<id>-<run>.md.
"""
import asyncio, json, os, subprocess, sys, time
from pathlib import Path

ARM, MODE, CASES, OUT, OUTDIR = sys.argv[1:6]
RUNS = int(sys.argv[6]) if len(sys.argv) > 6 else 1
TIMEOUT = 30
WEBCLAW = os.environ.get("WEBCLAW_BIN") or str(Path(__file__).resolve().parent / "bin/webclaw")


def webclaw(fmt):
    def run(case):
        src = ["--file", case["html_path"]] if MODE == "html" else [case["url"]]
        r = subprocess.run([WEBCLAW, *src, "-f", fmt, "-t", str(TIMEOUT)],
                           capture_output=True, text=True, timeout=TIMEOUT + 15)
        if r.returncode != 0 and not r.stdout:
            raise RuntimeError(r.stderr.strip()[-300:])
        return r.stdout
    return run


def trafilatura_arm(case):
    import trafilatura
    html = Path(case["html_path"]).read_text() if MODE == "html" else trafilatura.fetch_url(case["url"])
    if not html:
        raise RuntimeError("fetch failed")
    return trafilatura.extract(html, output_format="markdown", include_links=True,
                               include_tables=True, favor_recall=True) or ""


def scrapling_arm(case):
    if MODE == "url":
        from scrapling.fetchers import Fetcher
        page = Fetcher.get(case["url"], timeout=TIMEOUT)
        return page.markdown()
    from scrapling.parser import Selector
    from scrapling.core.shell import Convertor
    page = Selector(Path(case["html_path"]).read_text(), url=case["url"])
    page = Convertor._sanitize_for_ai(Convertor._strip_noise_tags(page))
    return Convertor._convert_to_markdown(page.html_content)


_CRAWLER = None


def crawl4ai_arm(fit):
    async def go(case):
        global _CRAWLER
        from crawl4ai import AsyncWebCrawler, CrawlerRunConfig, CacheMode
        from crawl4ai.markdown_generation_strategy import DefaultMarkdownGenerator
        from crawl4ai.content_filter_strategy import PruningContentFilter
        if _CRAWLER is None:
            _CRAWLER = AsyncWebCrawler()
            await _CRAWLER.start()
        gen = DefaultMarkdownGenerator(content_filter=PruningContentFilter()) if fit else DefaultMarkdownGenerator()
        cfg = CrawlerRunConfig(cache_mode=CacheMode.BYPASS, markdown_generator=gen, page_timeout=TIMEOUT * 1000)
        target = "raw:" + Path(case["html_path"]).read_text() if MODE == "html" else case["url"]
        res = await asyncio.wait_for(_CRAWLER.arun(target, config=cfg), TIMEOUT + 15)
        if not res.success:
            raise RuntimeError(res.error_message or "crawl failed")
        md = res.markdown
        return (md.fit_markdown if fit else md.raw_markdown) or ""
    return lambda case: asyncio.get_event_loop().run_until_complete(go(case))


ARMS = {
    "webclaw-llm": lambda: webclaw("llm"),
    "webclaw-md": lambda: webclaw("markdown"),
    "trafilatura": lambda: trafilatura_arm,
    "scrapling": lambda: scrapling_arm,
    "crawl4ai-raw": lambda: crawl4ai_arm(False),
    "crawl4ai-fit": lambda: crawl4ai_arm(True),
}


def main():
    fn = ARMS[ARM]()
    outdir = Path(OUTDIR)
    outdir.mkdir(parents=True, exist_ok=True)
    cases = [json.loads(l) for l in open(CASES)]
    for case in cases:
        if "html_path" in case:
            case["html_path"] = str(Path(CASES).resolve().parent / case["html_path"])
    with open(OUT, "a") as out:
        for run in range(RUNS):
            for case in cases:
                t0 = time.time()
                text, err = "", None
                try:
                    text = fn(case) or ""
                except Exception as e:
                    err = f"{type(e).__name__}: {e}"[:300]
                secs = time.time() - t0
                (outdir / f"{case['id']}-{run}.md").write_text(text)
                out.write(json.dumps({"arm": ARM, "mode": MODE, "id": case["id"], "run": run,
                                      "ok": bool(text.strip()) and err is None,
                                      "seconds": round(secs, 3), "error": err}) + "\n")
                out.flush()


if __name__ == "__main__":
    main()
