# PDFs 2026-10-15-pdf

Each tool prints 10 pages to PDF. `cold` starts a fresh browser per page, as one command per PDF does: `webclaw --save-pdf --print-background`, and Crawl4AI's `pdf=True` with a new crawler. `warm` keeps one browser open for every page: webclaw through its MCP server's `screenshot` tool with format `pdf` and `print_background`, Crawl4AI with one crawler, 2 rounds. Every arm prints US Letter with backgrounds. `ok` counts files that start with `%PDF-` and that webclaw's PDF reader opens with at least one page. `recall` is the share of the distinct words (4+ letters) of the page's content, as `webclaw --render always -f text` reads it once at the start, found in the PDF's text, averaged over pages. webclaw's PDF reader drops some right-to-left text, so recall is understated on pages that have it, for every tool alike.

| tool | mode | ok | ms (median) | ms (p95) | KB (median) | pages (median) | recall |
|---|---|---:|---:|---:|---:|---:|---:|
| webclaw | cold | 10/10 | 1771 | 2013 | 199 | 4.5 | 0.910 |
| crawl4ai | cold | 10/10 | 962 | 1384 | 193 | 4 | 0.906 |
| webclaw | warm | 20/20 | 936 | 1657 | 199 | 4.5 | 0.910 |
| crawl4ai | warm | 20/20 | 669 | 1499 | 193 | 4 | 0.906 |

scrapling: no PDF export: its fetchers, sessions and MCP tools return pages, text and screenshots only.

Cold run, per page: pages, KB and recall:

| page | reference words | webclaw | crawl4ai |
|---|---:|---|---|
| https://example.com/ | 76 | 1 p, 185 KB, 0.63 | 1 p, 185 KB, 0.63 |
| https://quotes.toscrape.com/js/ | 100 | 2 p, 79 KB, 0.99 | 2 p, 79 KB, 0.99 |
| https://books.toscrape.com/ | 71 | 7 p, 335 KB, 1.00 | 4 p, 346 KB, 1.00 |
| https://en.wikipedia.org/wiki/Web_scraping | 1219 | 10 p, 305 KB, 0.93 | 9 p, 297 KB, 0.93 |
| https://news.ycombinator.com/ | 224 | 2 p, 213 KB, 1.00 | 2 p, 202 KB, 1.00 |
| https://www.rust-lang.org/ | 201 | 5 p, 75 KB, 0.96 | 5 p, 75 KB, 0.96 |
| https://docs.python.org/3/library/asyncio.html | 153 | 3 p, 144 KB, 0.90 | 2 p, 143 KB, 0.90 |
| https://github.com/rust-lang/rust | 141 | 5 p, 509 KB, 0.95 | 5 p, 507 KB, 0.89 |
| https://developer.mozilla.org/en-US/docs/Web/HTML | 374 | 4 p, 150 KB, 0.96 | 4 p, 149 KB, 0.96 |
| https://www.bbc.com/news | 626 | 5 p, 4201 KB, 0.79 | 5 p, 4380 KB, 0.80 |
