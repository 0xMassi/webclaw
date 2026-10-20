# Selector benchmark 2026-10-13-xpath

Each tool reads the same 50 frozen books.toscrape.com catalogue pages and returns one record per book (title, price, rating) through its CSS selector API, and again through its XPath API (the `-xpath` rows). `correct` counts records equal to the values parsed from the raw HTML. Seconds are wall time for all pages. Python tools run in one process; webclaw runs its CLI once per page, and `of which start` is the time the same number of `webclaw --version` runs takes.

`synthetic ms` repeats the test from Scrapling's own benchmarks.py: the text of every `.item` (XPath: `//div[@class='item']`) in a page of 5,000 nested divs, mean wall time over 20 runs. webclaw and Crawl4AI return each element's whole text, descendants included, so they walk every nested div below each match; Scrapling's `::text` returns only the element's own text nodes, of which these divs have none. Crawl4AI returns the `class` attribute as a list, joined with spaces before scoring.

| arm | records | correct | seconds | of which start | ms/page | synthetic ms | synthetic values |
|---|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 1000/1000 | 1000 | 0.24 | 0.18 | 4.9 | 88.7 | 5000 |
| scrapling | 1000/1000 | 1000 | 0.07 | - | 1.3 | 1.7 | 0 |
| crawl4ai | 1000/1000 | 1000 | 0.57 | - | 11.4 | 18367.0 | 4999 |
| webclaw-xpath | 1000/1000 | 1000 | 0.28 | 0.19 | 5.6 | 509.6 | 5000 |
| scrapling-xpath | 1000/1000 | 1000 | 0.06 | - | 1.2 | 0.9 | 0 |
| crawl4ai-xpath | 1000/1000 | 1000 | 0.28 | - | 5.6 | 1.7 | 254 |
