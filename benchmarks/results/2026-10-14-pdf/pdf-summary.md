# PDFs 2026-10-14-pdf

Each tool prints 10 pages to PDF. `cold` starts a fresh browser per page, as one command per PDF does: `webclaw --save-pdf --print-background`, and Crawl4AI's `pdf=True` with a new crawler. `warm` keeps one browser open for every page: webclaw through its MCP server's `screenshot` tool with format `pdf` (default options, so no backgrounds), Crawl4AI with one crawler, 2 rounds. Both print US Letter. `ok` counts files that start with `%PDF-` and that webclaw's PDF reader opens with at least one page. `recall` is the share of the distinct words (4+ letters) of the page's content, as `webclaw --render always -f text` reads it once at the start, found in the PDF's text, averaged over pages.

| tool | mode | backgrounds | ok | ms (median) | ms (p95) | KB (median) | pages (median) | recall |
|---|---|---|---:|---:|---:|---:|---:|---:|
| webclaw | cold | yes | 10/10 | 1563 | 2099 | 275 | 4.5 | 0.910 |
| crawl4ai | cold | yes | 10/10 | 872 | 1467 | 194 | 4 | 0.905 |
| webclaw | warm | no | 20/20 | 994 | 1567 | 232 | 4.5 | 0.910 |
| crawl4ai | warm | yes | 20/20 | 642 | 1285 | 194 | 4 | 0.905 |

scrapling: no PDF export: its fetchers, sessions and MCP tools return pages, text and screenshots only.

Cold run, per page: pages, KB and recall:

| page | reference words | webclaw | crawl4ai |
|---|---:|---|---|
| https://example.com/ | 76 | 1 p, 187 KB, 0.63 | 1 p, 185 KB, 0.63 |
| https://quotes.toscrape.com/js/ | 100 | 2 p, 94 KB, 0.99 | 2 p, 79 KB, 0.99 |
| https://books.toscrape.com/ | 71 | 7 p, 411 KB, 1.00 | 4 p, 346 KB, 1.00 |
| https://en.wikipedia.org/wiki/Web_scraping | 1219 | 10 p, 517 KB, 0.93 | 9 p, 297 KB, 0.93 |
| https://news.ycombinator.com/ | 224 | 2 p, 352 KB, 1.00 | 2 p, 204 KB, 1.00 |
| https://www.rust-lang.org/ | 201 | 5 p, 101 KB, 0.96 | 5 p, 75 KB, 0.96 |
| https://docs.python.org/3/library/asyncio.html | 153 | 3 p, 182 KB, 0.90 | 2 p, 143 KB, 0.90 |
| https://github.com/rust-lang/rust | 141 | 5 p, 633 KB, 0.95 | 5 p, 505 KB, 0.89 |
| https://developer.mozilla.org/en-US/docs/Web/HTML | 374 | 4 p, 198 KB, 0.96 | 4 p, 149 KB, 0.96 |
| https://www.bbc.com/news | 629 | 5 p, 4347 KB, 0.79 | 5 p, 4372 KB, 0.79 |
