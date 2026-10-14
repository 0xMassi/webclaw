import json, os, subprocess, sys, time
from pathlib import Path

ARM, CORPUS, OUT = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
URLS = json.loads((CORPUS / "urls.json").read_text())


def webclaw(slug, url):
    out = subprocess.run([os.environ["WEBCLAW_BIN"], "--file", str(CORPUS / f"{slug}.html"), "--tables"],
                         capture_output=True, text=True, check=True).stdout
    return json.loads(out)


def crawl4ai():
    from crawl4ai import CrawlerRunConfig
    from crawl4ai.content_scraping_strategy import LXMLWebScrapingStrategy

    strategy = LXMLWebScrapingStrategy()
    params = CrawlerRunConfig().__dict__.copy()
    params.pop("url", None)

    def page(slug, url):
        result = strategy.scrap(url, (CORPUS / f"{slug}.html").read_text(), **params)
        return [{"caption": t.get("caption"), "headers": t["headers"], "rows": t["rows"]} for t in result.media.tables]

    return page


def scrapling():
    from scrapling import Selector

    def page(slug, url):
        tables = []
        for table in Selector((CORPUS / f"{slug}.html").read_text(), url=url, adaptive=False).css("table"):
            rows = [[cell.get_all_text(strip=True) for cell in tr.css("th, td")] for tr in table.css("tr")]
            if rows:
                tables.append({"headers": rows[0], "rows": rows[1:]})
        return tables

    return page


if ARM == "webclaw":
    for _ in range(3):
        subprocess.run([os.environ["WEBCLAW_BIN"], "--version"], capture_output=True, check=True)
    page = webclaw
else:
    page = {"crawl4ai": crawl4ai, "scrapling": scrapling}[ARM]()
start = time.perf_counter()
tables = {slug: page(slug, url) for slug, url in URLS.items()}
result = {"tables": tables, "seconds": time.perf_counter() - start}
if ARM == "webclaw":
    t = time.perf_counter()
    for _ in URLS:
        subprocess.run([os.environ["WEBCLAW_BIN"], "--version"], capture_output=True, check=True)
    result["start_seconds"] = time.perf_counter() - t
OUT.write_text(json.dumps(result))
