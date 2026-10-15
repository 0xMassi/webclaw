# Page archives archive-2026-10-15-v2

## MHTML

Each tool saves 10 pages as MHTML. `cold` starts a fresh browser per page, as one command per archive does: `webclaw --save-mhtml`, and Crawl4AI's `capture_mhtml=True` with a new crawler. `warm` keeps one browser open for every page: webclaw through its MCP server's `screenshot` tool with format `mhtml`, Crawl4AI with one crawler, 2 rounds. Every archive is then opened from disk (file://) in a fresh headless Chromium with the network off (an offline context that also aborts every non-file request) and compared with the live page as the same Chromium shows it at 1280x720 after the load event, network idle (at most 10 s) and 1 s, measured once at the start. `recall` is the share of the live page's distinct visible words (4+ letters, from `innerText`) that the offline copy shows. `images` counts loaded `<img>` elements offline against live, summed over pages.

| tool | mode | ok | ms (median) | ms (p95) | KB (median) | recall | images offline / live |
|---|---|---:|---:|---:|---:|---:|---:|
| webclaw | cold | 10/10 | 2336 | 2589 | 196 | 1.000 | 79 / 80 |
| crawl4ai | cold | 10/10 | 2868 | 2980 | 223 | 0.999 | 80 / 80 |
| webclaw | warm | 20/20 | 970 | 1306 | 197 | 0.998 | 158 / 160 |
| crawl4ai | warm | 20/20 | 2379 | 2893 | 223 | 0.994 | 160 / 160 |

scrapling: no MHTML or other page archive: its fetchers, sessions and MCP tools return pages, text and screenshots only.

Cold run, per page: KB, recall, images offline / live:

| page | live words | live images | webclaw | crawl4ai |
|---|---:|---:|---|---|
| https://example.com/ | 76 | 0 | 5 KB, 1.00, 0/0 | 5 KB, 1.00, 0/0 |
| https://quotes.toscrape.com/js/ | 106 | 0 | 156 KB, 1.00, 0/0 | 155 KB, 1.00, 0/0 |
| https://books.toscrape.com/ | 113 | 20 | 488 KB, 1.00, 20/20 | 488 KB, 1.00, 20/20 |
| https://en.wikipedia.org/wiki/Web_scraping | 1243 | 6 | 807 KB, 1.00, 5/6 | 808 KB, 0.99, 6/6 |
| https://news.ycombinator.com/ | 226 | 2 | 53 KB, 1.00, 2/2 | 53 KB, 1.00, 2/2 |
| https://www.rust-lang.org/ | 237 | 9 | 171 KB, 1.00, 9/9 | 171 KB, 1.00, 9/9 |
| https://docs.python.org/3/library/asyncio.html | 220 | 3 | 84 KB, 1.00, 3/3 | 84 KB, 1.00, 3/3 |
| https://github.com/rust-lang/rust | 317 | 18 | 3290 KB, 1.00, 18/18 | 3290 KB, 1.00, 18/18 |
| https://developer.mozilla.org/en-US/docs/Web/HTML | 399 | 0 | 221 KB, 1.00, 0/0 | 275 KB, 1.00, 0/0 |
| https://www.bbc.com/news | 573 | 22 | 513 KB, 1.00, 22/22 | 567 KB, 1.00, 22/22 |

## WARC

`webclaw URL -f text --warc FILE.warc.gz` on each page, and a crawl of https://books.toscrape.com/ (`--crawl --depth 2 --max-pages 30`). Each file is checked by a WARC reader written for this benchmark (Python standard library only): every gzip member holds one record, the first record is `warcinfo`, every record has a unique `urn:uuid` id, a UTC date and a matching `Content-Length`, every block digest and every response payload digest (SHA-1, base32) matches, each response starts with an HTTP status line, each request record points at its response, and every record points at a `warcinfo` record. `ok` also needs a non-empty 2xx response for the page. `ms without` is the same command without `--warc`, run just before.

| page | ok | ms | ms without | KB | records | digests checked |
|---|---|---:|---:|---:|---:|---:|
| https://example.com/ | yes | 1614 | 1849 | 2 | 3 | 4 |
| https://quotes.toscrape.com/js/ | yes | 2748 | 2829 | 3 | 3 | 4 |
| https://books.toscrape.com/ | yes | 528 | 548 | 6 | 3 | 4 |
| https://en.wikipedia.org/wiki/Web_scraping | yes | 171 | 174 | 49 | 3 | 4 |
| https://news.ycombinator.com/ | yes | 638 | 604 | 7 | 3 | 4 |
| https://www.rust-lang.org/ | yes | 175 | 144 | 8 | 5 | 7 |
| https://docs.python.org/3/library/asyncio.html | yes | 93 | 95 | 7 | 3 | 4 |
| https://github.com/rust-lang/rust | yes | 210 | 256 | 52 | 3 | 4 |
| https://developer.mozilla.org/en-US/docs/Web/HTML | yes | 99 | 107 | 28 | 3 | 4 |
| https://www.bbc.com/news | yes | 139 | 139 | 66 | 3 | 4 |

Pages: 10/10 ok, median 193 ms with `--warc` against 215 ms without.

Crawl: ok, 30 pages crawled in 2.6 s, 31 response records (30 distinct URLs answered under 400, the rest robots.txt and redirects), 63 records, 94 digests checked, 166 KB.

crawl4ai: no WARC output: it archives a page only as MHTML (`capture_mhtml`), and its crawls return results, not HTTP records.
scrapling: no WARC output: its spider writes items, and responses are not kept as HTTP records.
