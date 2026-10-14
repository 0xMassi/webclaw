import importlib.metadata
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

import webclaw_cli as wc

SHOP = """<html><head><title>Shop</title></head><body><main>
<h1>Garden tools</h1>
<p>Our garden tools are forged from carbon steel and sharpened by hand in the workshop.</p>
<ul>
<li class="item"><a href="/a">Spade</a> <span class="price">12</span></li>
<li class="item"><a href="/b">Rake</a> <span class="price">9</span></li>
<li class="item"><a href="/c">Hoe</a> <span class="price">7</span></li>
</ul>
<table><caption>Prices</caption><tr><th>Tool</th><th>Price</th></tr>
<tr><td>Spade</td><td>12</td></tr><tr><td>Rake</td><td>9</td></tr></table>
</main></body></html>"""


class Offline(unittest.TestCase):
    def test_binary_runs(self):
        out = subprocess.run([wc.binary_path(), "--version"], capture_output=True, text=True, check=True)
        self.assertTrue(out.stdout.startswith("webclaw "))

    def test_binary_is_the_packaged_one(self):
        with tempfile.TemporaryDirectory() as d, mock.patch.dict(os.environ, {"PATH": d}):
            Path(d, "webclaw").write_text("#!/bin/sh\necho fake\n")
            Path(d, "webclaw").chmod(0o755)
            wc.binary_path.cache_clear()
            path = wc.binary_path()
        recorded = {str(Path(f.locate()).resolve()) for f in importlib.metadata.files("webclaw-cli") or []}
        self.assertIn(path, recorded)

    def test_scrape_formats(self):
        self.assertIn("# Garden tools", wc.scrape(SHOP))
        self.assertIn("carbon steel", wc.scrape(SHOP, "text"))
        self.assertIn("Garden tools", wc.scrape(SHOP, "llm"))
        result = wc.scrape(SHOP, "json")
        self.assertEqual(result["metadata"]["title"], "Shop")
        self.assertIn("Garden tools", result["content"]["markdown"])

    def test_scrape_options(self):
        out = wc.scrape(SHOP, exclude=["ul", "table"])
        self.assertIn("carbon steel", out)
        self.assertNotIn("Spade", out)

    def test_select(self):
        self.assertEqual(wc.select(SHOP, css=".item a"), ["Spade", "Rake", "Hoe"])
        self.assertEqual(wc.select(SHOP, xpath="//li/span/text()"), ["12", "9", "7"])
        self.assertEqual(wc.select(SHOP, css=".item a::attr(href)"), ["/a", "/b", "/c"])
        records = wc.select(SHOP, css=".item", fields={"name": "a", "price": ".price"})
        self.assertEqual(records[0], {"name": "Spade", "price": "12"})

    def test_tables(self):
        found = wc.tables(SHOP)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0]["caption"], "Prices")
        self.assertEqual(found[0]["headers"], ["Tool", "Price"])
        self.assertEqual(found[0]["rows"], [["Spade", "12"], ["Rake", "9"]])

    def test_find_text_and_similar(self):
        found = wc.find_text(SHOP, "rake")
        self.assertEqual([f["text"] for f in found], ["Rake", "Rake"])
        self.assertEqual(wc.select(SHOP, css=found[0]["selector"]), ["Rake"])
        self.assertEqual(wc.find_text(SHOP, "rake", case_sensitive=True), [])
        self.assertEqual([f["text"] for f in wc.find_text(SHOP, r"^\d+$", regex=True)], ["12", "9", "7", "12", "9"])
        like = wc.similar(SHOP, text="Rake")
        self.assertEqual([m["text"] for m in like["matches"]], ["Spade", "Rake", "Hoe"])
        records = wc.similar(SHOP, css=".item", fields={"price": ".price"})
        self.assertEqual([r["price"] for r in records["records"]], ["12", "9", "7"])

    def test_error_carries_stderr_and_code(self):
        with self.assertRaises(wc.WebclawError) as cm:
            wc.select(SHOP, css="[[[")
        self.assertEqual(cm.exception.returncode, 1)
        self.assertIn(str(cm.exception), cm.exception.stderr)
        self.assertNotIn("error: ", str(cm.exception))

    def test_error_is_the_error_line(self):
        with self.assertRaises(wc.WebclawError) as cm:
            wc.find_text(SHOP, "x", regex=True, exact=True)
        self.assertEqual(cm.exception.returncode, 2)
        self.assertIn("--exact", str(cm.exception))

    def test_target_is_never_a_flag(self):
        with self.assertRaises(wc.WebclawError):
            wc.scrape("--version")

    def test_option_flags(self):
        flags = wc._options({"proxy": "http://p:1", "browser": "firefox", "headers": {"X-A": "1"},
                             "cookies": {"a": "1", "b": "2"}, "timeout": 5, "render": "never", "cache": True})
        self.assertEqual(flags, ["--proxy=http://p:1", "--browser=firefox", "--header=X-A: 1",
                                 "--cookie=a=1; b=2", "--timeout=5", "--render=never", "--cache"])
        self.assertEqual(wc._options({"cookies": "c=3"}), ["--cookie=c=3"])

    def test_star_import_keeps_builtin_map(self):
        names = {}
        exec("from webclaw_cli import *", names)
        self.assertNotIn("map", names)
        self.assertIn("crawl", names)

    def test_bad_arguments(self):
        with self.assertRaises(TypeError):
            wc.scrape(SHOP, proxies="x")
        with self.assertRaises(ValueError):
            wc.select(SHOP, css="a", xpath="//a")
        with self.assertRaises(ValueError):
            wc.similar(SHOP)


def chrome_left() -> set:
    out = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True, text=True).stdout
    procs = {line.split(None, 1)[0] for line in out.splitlines() if "webclaw-chrome-" in line}
    return procs | {p.name for p in Path(tempfile.gettempdir()).glob("webclaw-chrome-*")}


@unittest.skipUnless(os.environ.get("WEBCLAW_REAL_SITES"), "set WEBCLAW_REAL_SITES=1 to run against public sites")
class RealSites(unittest.TestCase):
    def test_scrape(self):
        self.assertIn("documentation examples", wc.scrape("https://example.com/"))
        self.assertEqual(wc.scrape("https://example.com/", "json")["metadata"]["title"], "Example Domain")

    def test_http_error(self):
        with self.assertRaises(wc.WebclawError) as cm:
            wc.scrape("https://books.toscrape.com/no-such-page")
        self.assertIn("404", str(cm.exception))

    def test_headers_and_cookies(self):
        out = wc.scrape("https://httpbin.org/headers", "text", headers={"X-Test": "yes-please"},
                        cookies={"a": "1", "b": "2"})
        self.assertIn("yes-please", out)
        self.assertIn("a=1; b=2", out)

    def test_cache(self):
        with tempfile.TemporaryDirectory() as d, mock.patch.dict(os.environ, {"WEBCLAW_CACHE_DIR": d}):
            first = wc.scrape("https://example.com/", cache=True)
            self.assertTrue(any(Path(d).rglob("*")))
            self.assertEqual(wc.scrape("https://example.com/", cache=True), first)

    def test_select(self):
        titles = wc.select("https://books.toscrape.com/", css="h3 a::attr(title)")
        self.assertEqual(len(titles), 20)
        self.assertIn("A Light in the Attic", titles)

    def test_tables(self):
        found = wc.tables("https://en.wikipedia.org/wiki/List_of_countries_by_population_(United_Nations)")
        self.assertTrue(any(len(t["rows"]) > 100 for t in found))

    def test_crawl(self):
        pages = list(wc.crawl("https://quotes.toscrape.com/", max_pages=3))
        self.assertEqual(len(pages), 3)
        self.assertTrue(all(p["extraction"]["content"]["markdown"] for p in pages))

    def test_crawl_stops_with_the_loop(self):
        pages = wc.crawl("https://quotes.toscrape.com/", max_pages=50)
        self.assertEqual(next(pages)["url"], "https://quotes.toscrape.com/")
        pages.close()

    @unittest.skipIf(sys.platform == "win32", "uses ps")
    def test_stopping_a_rendering_crawl_stops_chrome(self):
        before = chrome_left()
        pages = wc.crawl("https://quotes.toscrape.com/", max_pages=30, render="always")
        next(pages)
        started = chrome_left() - before
        pages.close()
        time.sleep(1)
        self.assertTrue(started)
        self.assertEqual(chrome_left() - before, set())

    @unittest.skipIf(sys.platform == "win32", "uses SIGALRM")
    def test_interrupting_a_call_stops_chrome(self):
        def interrupt(*_):
            raise KeyboardInterrupt

        before = chrome_left()
        old = signal.signal(signal.SIGALRM, interrupt)
        signal.setitimer(signal.ITIMER_REAL, 4)
        try:
            with self.assertRaises(KeyboardInterrupt):
                wc.scrape("https://httpbin.org/delay/10", render="always", timeout=30)
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, old)
        time.sleep(1)
        self.assertEqual(chrome_left() - before, set())

    def test_crawl_error(self):
        with self.assertRaises(wc.WebclawError):
            list(wc.crawl("https://no-such-host.invalid/"))

    def test_map(self):
        urls = wc.map("https://www.rust-lang.org/", limit=10)
        self.assertEqual(len(urls), 10)
        self.assertTrue(all("rust-lang.org/" in u for u in urls))

    def test_render(self):
        self.assertIn("Albert Einstein", wc.scrape("https://quotes.toscrape.com/js/", render="always"))

    def test_screenshot(self):
        with tempfile.TemporaryDirectory() as d:
            png = wc.screenshot("https://example.com/", Path(d) / "shot.png")
            self.assertEqual(png.read_bytes()[:8], b"\x89PNG\r\n\x1a\n")
            jpg = wc.screenshot("https://example.com/", os.path.join(d, "shot.jpg"), full_page=True)
            self.assertEqual(jpg.read_bytes()[:2], b"\xff\xd8")


if __name__ == "__main__":
    unittest.main()
