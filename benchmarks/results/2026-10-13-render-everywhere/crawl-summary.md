# Crawl benchmark 2026-10-13-render-everywhere

Each arm crawls each site with the same page cap and depth, concurrency 8, robots.txt obeyed. Medians over rounds. `disallowed` counts fetched URLs that the site's robots.txt disallows and that returned content, judged by Python's urllib.robotparser.

`pages ok` can exceed the cap when a tool does not enforce it. `content found` counts, on sites that build their content with JavaScript, the expected items (quotes) present in the crawl's markdown.

| site | arm | rounds | pages ok | content found | seconds | pages/s | peak MB | duplicates | disallowed |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| books.toscrape.com | webclaw | 3 | 300 | - | 6.7 | 44.7 | 58 | 0 | 0 |
| books.toscrape.com | scrapling | 3 | 300 | - | 8.1 | 36.9 | 81 | 0 | 0 |
| books.toscrape.com | crawl4ai | 3 | 586 | - | 67.0 | 8.8 | 1481 | 0 | 0 |
| scrapethissite.com | webclaw | 3 | 33 | - | 3.4 | 9.6 | 1378 | 0 | 0 |
| scrapethissite.com | scrapling | 3 | 35 | - | 2.1 | 16.4 | 85 | 0 | 0 |
| scrapethissite.com | crawl4ai | 3 | 34 | - | 6.2 | 5.5 | 1518 | 1 | 0 |
| quotes.toscrape.com-js | webclaw | 3 | 11 | 100/100 | 7.3 | 1.5 | 1476 | 0 | 0 |
| quotes.toscrape.com-js | scrapling | 3 | 11 | 100/100 | 4.9 | 2.2 | 1438 | 0 | 0 |
| quotes.toscrape.com-js | crawl4ai | 3 | 11 | 100/100 | 10.3 | 1.1 | 737 | 0 | 0 |

## Kill and resume

Each tool crawls books.toscrape.com (cap 200, depth 4, concurrency 8), streaming pages to a file, through its own resume mechanism. After 3s the whole process tree gets SIGKILL, then the same command reruns until it exits cleanly. `unique` counts distinct pages in the output; `duplicates` counts pages written twice.

| arm | pages before kill | unique pages at end | duplicates | seconds after restart | runs after kill |
|---|---:|---:|---:|---:|---:|
| webclaw | 107 | 200 | 32 | 3.1 | 1 |
| scrapling | 116 | 200 | 116 | 4.5 | 1 |
| crawl4ai | 2 | 2 | 0 | 0.6 | 1 |
