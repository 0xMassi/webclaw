"""One citations arm over the frozen html cases. Runs inside that arm's own venv.

usage: citations_arms.py <webclaw|crawl4ai> <html|url> <cases-html.jsonl> <out.jsonl> <rounds>
Each row: id, plain, cited, seconds_plain, seconds_cited, seconds_step (crawl4ai only), error.
"""
import asyncio, json, os, statistics, subprocess, sys, time
from pathlib import Path

ARM, MODE, CASES, OUT, ROUNDS = sys.argv[1], sys.argv[2], Path(sys.argv[3]), Path(sys.argv[4]), int(sys.argv[5])
CASE_LIST = [json.loads(l) for l in open(CASES)]


def timed(fn):
    t = time.perf_counter()
    value = fn()
    return value, time.perf_counter() - t


def webclaw(case):
    src = ["--file", str(CASES.parent / case["html_path"])] if MODE == "html" else [case["url"], "-t", "30"]

    def run(*extra):
        r = subprocess.run([os.environ["WEBCLAW_BIN"], *src, "-f", "markdown", *extra],
                           capture_output=True, text=True, timeout=60)
        if r.returncode:
            raise RuntimeError((r.stderr.strip().splitlines() or [""])[-1][-300:])
        return r.stdout

    plain, cited, tp, tc = None, None, [], []
    for _ in range(ROUNDS):
        plain, s = timed(run)
        tp.append(s)
        cited, s = timed(lambda: run("--citations"))
        tc.append(s)
    return {"plain": plain, "cited": cited, "seconds_plain": statistics.median(tp),
            "seconds_cited": statistics.median(tc)}


_CRAWLER = None


def crawl4ai(case):
    from crawl4ai import AsyncWebCrawler, CrawlerRunConfig, CacheMode
    from crawl4ai.markdown_generation_strategy import DefaultMarkdownGenerator

    async def page():
        global _CRAWLER
        if _CRAWLER is None:
            _CRAWLER = AsyncWebCrawler()
            await _CRAWLER.start()
        cfg = CrawlerRunConfig(cache_mode=CacheMode.BYPASS, markdown_generator=DefaultMarkdownGenerator())
        target = "raw:" + (CASES.parent / case["html_path"]).read_text() if MODE == "html" else case["url"]
        res = await asyncio.wait_for(_CRAWLER.arun(target, config=cfg), 45)
        if not res.success:
            raise RuntimeError(res.error_message or "crawl failed")
        return res

    loop = asyncio.get_event_loop()
    times, step = [], []
    for _ in range(ROUNDS):
        res, s = timed(lambda: loop.run_until_complete(page()))
        times.append(s)
        base = "" if MODE == "html" else res.redirected_url or case["url"]
        md = res.markdown if MODE == "url" else DefaultMarkdownGenerator().generate_markdown(res.cleaned_html, base_url=base)
        (body, refs), s = timed(lambda: DefaultMarkdownGenerator().convert_links_to_citations(md.raw_markdown, base))
        step.append(s)
    return {"plain": md.raw_markdown, "cited": md.markdown_with_citations + md.references_markdown,
            "seconds_plain": statistics.median(times), "seconds_cited": statistics.median(times),
            "seconds_step": statistics.median(step)}


def main():
    fn = {"webclaw": webclaw, "crawl4ai": crawl4ai}[ARM]
    with open(OUT, "w") as out:
        for case in CASE_LIST:
            row = {"id": case["id"], "error": None}
            try:
                row.update(fn(case))
            except Exception as e:
                row["error"] = f"{type(e).__name__}: {e}"[:300]
            out.write(json.dumps(row) + "\n")
            out.flush()


if __name__ == "__main__":
    main()
