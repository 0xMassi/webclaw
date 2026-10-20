import json, os, statistics, subprocess, sys, time
from pathlib import Path

ARM, CORPUS, OUT = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
PAGES = sorted(CORPUS.glob("page-*.html"))
BASE = "article.product_pod"
FIELDS = {"title": "h3 a::attr(title)", "price": ".price_color", "rating": "p.star-rating::attr(class)"}
XBASE = "//article[@class='product_pod']"
XFIELDS = {"title": ".//h3/a/@title", "price": ".//p[@class='price_color']",
           "rating": ".//p[contains(@class,'star-rating')]/@class"}
XITEM = "//div[@class='item']"
SYNTHETIC = "<html><body>" + '<div class="item">' * 5000 + "</div>" * 5000 + "</body></html>"
REPEATS = 20


def webclaw():
    bin_ = os.environ["WEBCLAW_BIN"]
    fields = [arg for name, sel in FIELDS.items() for arg in ("--field", f"{name}={sel}")]
    records = [json.loads(subprocess.run([bin_, "--file", str(p), "--select", BASE, *fields],
                                         capture_output=True, text=True, check=True).stdout) for p in PAGES]

    def synthetic():
        return json.loads(subprocess.run([bin_, "--stdin", "--select", ".item"], input=SYNTHETIC,
                                         capture_output=True, text=True, check=True).stdout)

    return records, synthetic


def webclaw_xpath():
    bin_ = os.environ["WEBCLAW_BIN"]
    fields = [arg for name, sel in XFIELDS.items() for arg in ("--field", f"{name}={sel}")]
    records = [json.loads(subprocess.run([bin_, "--file", str(p), "--xpath", XBASE, *fields],
                                         capture_output=True, text=True, check=True).stdout) for p in PAGES]

    def synthetic():
        return json.loads(subprocess.run([bin_, "--stdin", "--xpath", XITEM], input=SYNTHETIC,
                                         capture_output=True, text=True, check=True).stdout)

    return records, synthetic


def scrapling_xpath():
    from scrapling import Selector

    def page(p):
        doc = Selector(p.read_text(), adaptive=False)
        return [{"title": pod.xpath(".//h3/a/@title").get(),
                 "price": pod.xpath(".//p[@class='price_color']/text()").get(),
                 "rating": pod.xpath(".//p[contains(@class,'star-rating')]/@class").get()}
                for pod in doc.xpath(XBASE)]

    return [page(p) for p in PAGES], lambda: Selector(SYNTHETIC, adaptive=False).xpath(XITEM + "/text()").getall()


def crawl4ai_xpath():
    from crawl4ai import JsonXPathExtractionStrategy

    schema = {"name": "books", "baseSelector": XBASE, "fields": [
        {"name": "title", "selector": ".//h3/a", "type": "attribute", "attribute": "title"},
        {"name": "price", "selector": ".//p[@class='price_color']", "type": "text"},
        {"name": "rating", "selector": ".//p[contains(@class,'star-rating')]", "type": "attribute",
         "attribute": "class"}]}
    strategy = JsonXPathExtractionStrategy(schema)
    items = JsonXPathExtractionStrategy({"name": "items", "baseSelector": XITEM, "fields": [
        {"name": "text", "selector": ".", "type": "text"}]})
    return ([strategy.extract("https://books.toscrape.com/", p.read_text()) for p in PAGES],
            lambda: items.extract("https://example.com/", SYNTHETIC))


def scrapling():
    from scrapling import Selector

    def page(p):
        doc = Selector(p.read_text(), adaptive=False)
        return [{"title": pod.css("h3 a::attr(title)").get(), "price": pod.css(".price_color::text").get(),
                 "rating": pod.css("p.star-rating::attr(class)").get()} for pod in doc.css(BASE)]

    return [page(p) for p in PAGES], lambda: Selector(SYNTHETIC, adaptive=False).css(".item::text").getall()


def crawl4ai():
    from crawl4ai import JsonCssExtractionStrategy

    schema = {"name": "books", "baseSelector": BASE, "fields": [
        {"name": "title", "selector": "h3 a", "type": "attribute", "attribute": "title"},
        {"name": "price", "selector": ".price_color", "type": "text"},
        {"name": "rating", "selector": "p.star-rating", "type": "attribute", "attribute": "class"}]}
    strategy = JsonCssExtractionStrategy(schema)
    items = JsonCssExtractionStrategy({"name": "items", "baseSelector": ".item", "fields": [
        {"name": "text", "selector": ".item", "type": "text"}]})
    return ([strategy.extract("https://books.toscrape.com/", p.read_text()) for p in PAGES],
            lambda: items.extract("https://example.com/", SYNTHETIC))


if ARM.startswith("webclaw"):
    for _ in range(3):
        subprocess.run([os.environ["WEBCLAW_BIN"], "--version"], capture_output=True, check=True)
start = time.perf_counter()
records, synthetic = {"webclaw": webclaw, "scrapling": scrapling, "crawl4ai": crawl4ai,
                      "webclaw-xpath": webclaw_xpath, "scrapling-xpath": scrapling_xpath,
                      "crawl4ai-xpath": crawl4ai_xpath}[ARM]()
seconds = time.perf_counter() - start
matches = len(synthetic())
times = []
for _ in range(REPEATS):
    t = time.perf_counter()
    synthetic()
    times.append(time.perf_counter() - t)
result = {"records": records, "seconds": seconds, "synthetic_ms": 1000 * statistics.mean(times),
          "synthetic_values": matches}
if ARM.startswith("webclaw"):
    t = time.perf_counter()
    for _ in PAGES:
        subprocess.run([os.environ["WEBCLAW_BIN"], "--version"], capture_output=True, check=True)
    result["start_seconds"] = time.perf_counter() - t
OUT.write_text(json.dumps(result))
