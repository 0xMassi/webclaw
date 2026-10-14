# Page actions actions-2026-10-14

Each tool loads 9 pages that need interaction in a fresh headless browser, runs the same steps, and returns the page's HTML; 2 rounds. `items` counts the elements the case's selector finds in that HTML (the median over rounds), `ok` the rounds with every expected item, `s` the median wall time. `webclaw-none` renders with no actions. Ground truth and how it was checked are in `actions-cases.json`.

| case | kind | truth | webclaw items | webclaw ok | webclaw s | webclaw-none items | webclaw-none ok | webclaw-none s | scrapling items | scrapling ok | scrapling s | crawl4ai items | crawl4ai ok | crawl4ai s |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| quotes-scroll | infinite scroll | 100 | 100 | 2/2 | 7.3 | 10 | 0/2 | 2.0 | 100 | 2/2 | 12.3 | 100 | 2/2 | 7.5 |
| shop-scroll | infinite scroll | 117 | 117 | 2/2 | 15.8 | 3 | 0/2 | 1.4 | 117 | 2/2 | 41.3 | 13.5 | 0/2 | 2.1 |
| shop-load-more | load-more button | 117 | 117 | 2/2 | 7.0 | 6 | 0/2 | 1.3 | 117 | 2/2 | 21.5 | 117 | 2/2 | 19.9 |
| shop-ajax-page-3 | click to paginate | 6 | 6 | 2/2 | 2.4 | 5 | 0/2 | 1.3 | 6 | 2/2 | 3.0 | 6 | 2/2 | 1.8 |
| quotes-delayed | wait for late content | 10 | 10 | 2/2 | 12.0 | 0 | 0/2 | 2.0 | 10 | 2/2 | 11.6 | 10 | 2/2 | 11.6 |
| quotes-login | form | 10 | 10 | 2/2 | 2.7 | 0 | 0/2 | 1.6 | 10 | 2/2 | 3.3 | 10 | 2/2 | 1.8 |
| yc-directory | infinite scroll (real site) | 145 | 145 | 2/2 | 13.9 | 40 | 0/2 | 7.5 | 145 | 2/2 | 16.2 | 145 | 2/2 | 7.4 |
| github-topic | load-more button (real site) | 80 | 80 | 2/2 | 5.9 | 20 | 0/2 | 1.7 | 80 | 2/2 | 6.0 | 80 | 2/2 | 5.0 |
| npr-news | load-more button (real site) | 72 | 72 | 2/2 | 17.3 | 24 | 0/2 | 4.2 | 24 | 0/2 | 71.4 | 24 | 0/2 | 1.4 |
| **all** | | |  | **18/18** | 7.8 |  | **0/18** | 1.8 |  | **16/18** | 12.3 |  | **14/18** | 5.0 |
