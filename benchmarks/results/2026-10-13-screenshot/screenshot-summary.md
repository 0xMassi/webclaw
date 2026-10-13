# Screenshots 2026-10-13-screenshot

Each tool screenshots 10 pages as PNG. `cold` starts a fresh browser per page, as one command per screenshot does. `warm` keeps one browser open for every page: webclaw and Scrapling through their MCP servers' `screenshot` tools (Scrapling inside a browser session it opens first), Crawl4AI with one crawler, 2 rounds. `ok` counts images that are not blank (more than 3 colours in a 64x64 thumbnail). Crawl4AI's full-page capture stitches JPEG tiles.

| tool | mode | full page | ok | ms (median) | ms (p95) | KB (median) | most common size |
|---|---|---|---:|---:|---:|---:|---|
| webclaw | cold | no | 10/10 | 1741 | 2150 | 142 | 1280x720 |
| scrapling | cold | no | 10/10 | 999 | 1437 | 314 | 2560x1440 |
| crawl4ai | cold | no | 10/10 | 703 | 1378 | 101 | 1080x600 |
| webclaw | cold | yes | 10/10 | 1495 | 1899 | 507 | 1280x775 |
| scrapling | cold | yes | 10/10 | 1507 | 1777 | 1123 | 2560x1550 |
| crawl4ai | cold | yes | 10/10 | 3118 | 3686 | 1394 | 1080x745 |
| webclaw | warm | no | 20/20 | 894 | 1469 | 142 | 1280x720 |
| scrapling | warm | no | 20/20 | 588 | 1349 | 314 | 2560x1440 |
| crawl4ai | warm | no | 20/20 | 474 | 1169 | 101 | 1080x600 |

Full-page height in pixels, per page:

| page | webclaw | scrapling | crawl4ai |
|---|---:|---:|---:|
| https://example.com/ | 775 | 1550 | 745 |
| https://quotes.toscrape.com/js/ | 1698 | 3396 | 1723 |
| https://books.toscrape.com/ | 2569 | 5138 | 2569 |
| https://en.wikipedia.org/wiki/Web_scraping | 10474 | 20948 | 8265 |
| https://news.ycombinator.com/ | 1249 | 2498 | 1249 |
| https://www.rust-lang.org/ | 3490 | 6980 | 3567 |
| https://docs.python.org/3/library/asyncio.html | 2671 | 5342 | 2671 |
| https://github.com/rust-lang/rust | 3603 | 7206 | 3778 |
| https://developer.mozilla.org/en-US/docs/Web/HTML | 5322 | 10644 | 6192 |
| https://www.bbc.com/news | 6548 | 13096 | 6610 |
