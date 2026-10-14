# Page actions actions-2026-10-15

Each tool loads 9 pages that need interaction in a fresh headless browser, runs the same steps, and returns the page's HTML; 2 rounds. `items` counts the elements the case's selector finds in that HTML (the median over rounds), `ok` the rounds with every expected item, `s` the median wall time. `webclaw-none` renders with no actions. Ground truth and how it was checked are in `actions-cases.json`.

| case | kind | truth | webclaw items | webclaw ok | webclaw s | webclaw-none items | webclaw-none ok | webclaw-none s | scrapling items | scrapling ok | scrapling s | crawl4ai items | crawl4ai ok | crawl4ai s |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| quotes-scroll | infinite scroll | 100 | 100 | 2/2 | 7.1 | 10 | 0/2 | 2.2 | 100 | 2/2 | 12.5 | 100 | 2/2 | 30.0 |
| shop-scroll | infinite scroll | 117 | 117 | 2/2 | 15.6 | 3 | 0/2 | 1.4 | 117 | 2/2 | 43.1 | 117 | 2/2 | 41.7 |
| shop-load-more | load-more button | 117 | 117 | 2/2 | 8.2 | 6 | 0/2 | 1.5 | 117 | 2/2 | 32.9 | 117 | 2/2 | 32.0 |
| shop-ajax-page-3 | click to paginate | 6 | 6 | 2/2 | 2.2 | 5 | 0/2 | 1.4 | 6 | 2/2 | 3.0 | 6 | 2/2 | 2.1 |
| quotes-delayed | wait for late content | 10 | 10 | 2/2 | 12.0 | 0 | 0/2 | 2.1 | 10 | 2/2 | 11.9 | 10 | 2/2 | 11.7 |
| quotes-login | form | 10 | 10 | 2/2 | 2.6 | 0 | 0/2 | 1.6 | 10 | 2/2 | 3.3 | 10 | 2/2 | 1.8 |
| yc-directory | infinite scroll (real site) | 145 | 145 | 2/2 | 8.1 | 40 | 0/2 | 4.6 | 145 | 2/2 | 11.7 | 145 | 2/2 | 29.8 |
| github-topic | load-more button (real site) | 80 | 80 | 2/2 | 7.7 | 20 | 0/2 | 1.5 | 80 | 2/2 | 7.5 | 80 | 2/2 | 6.5 |
| npr-news | load-more button (real site) | 72 | 72 | 2/2 | 17.0 | 24 | 0/2 | 4.0 | 24 | 0/2 | 64.5 | 24 | 0/2 | 5.2 |
| **all** | | |  | **18/18** | 8.1 |  | **0/18** | 1.7 |  | **16/18** | 12.0 |  | **16/18** | 11.7 |
