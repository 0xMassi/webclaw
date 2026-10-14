# Question-guided crawl benchmark 2026-10-14-query-crawl

Each question names the page that answers it. Every arm crawls from the site's start page with a budget of 50 pages, depth 5, concurrency 4, the same path prefix, robots.txt obeyed. `answer by page N` counts questions whose answer page was among the first N pages the tool fetched. `median pages to answer` counts a miss as 51. `answer in output top 3` counts questions whose answer page is in the first 3 pages of the tool's own ranking (breadth-first arms: crawl order). `median seconds to answer` covers found answers only.

## All sites

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 60 | 0 | 2 | 4 | 51+ | 0 | 2.0 | 2.9 | 50 |
| webclaw-query | 60 | 32 | 51 | 55 | 10 | 53 | 0.6 | 1.6 | 50 |
| scrapling-bfs | 60 | 0 | 1 | 3 | 51+ | 0 | 1.8 | 2.6 | 50 |
| crawl4ai-bfs | 60 | 0 | 1 | 3 | 51+ | 0 | 7.4 | 10.0 | 49 |
| crawl4ai-best-first | 60 | 18 | 37 | 46 | 14 | 33 | 4.1 | 9.3 | 50 |
| crawl4ai-adaptive | 60 | 2 | 3 | 3 | 51+ | 3 | 3.6 | 2.2 | 7 |

## docs.python.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 3.0 | 50 |
| webclaw-query | 15 | 8 | 13 | 15 | 10 | 13 | 0.4 | 1.7 | 50 |
| scrapling-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 2.4 | 50 |
| crawl4ai-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 11.1 | 49 |
| crawl4ai-best-first | 15 | 0 | 9 | 11 | 14 | 7 | 4.1 | 9.5 | 50 |
| crawl4ai-adaptive | 15 | 0 | 1 | 1 | 51+ | 1 | 4.7 | 4.5 | 19 |

## www.postgresql.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 3.4 | 50 |
| webclaw-query | 15 | 7 | 14 | 14 | 12 | 14 | 0.6 | 1.2 | 50 |
| scrapling-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 9.9 | 50 |
| crawl4ai-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 8.8 | 49 |
| crawl4ai-best-first | 15 | 0 | 7 | 10 | 28 | 4 | 5.0 | 8.8 | 50 |
| crawl4ai-adaptive | 15 | 2 | 2 | 2 | 51+ | 2 | 2.9 | 1.6 | 5 |

## developer.mozilla.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 1.2 | 50 |
| webclaw-query | 15 | 14 | 15 | 15 | 4 | 15 | 0.2 | 0.9 | 50 |
| scrapling-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 1.1 | 50 |
| crawl4ai-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 8.5 | 49 |
| crawl4ai-best-first | 15 | 15 | 15 | 15 | 2 | 12 | 2.4 | 8.0 | 50 |
| crawl4ai-adaptive | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 0.9 | 2 |

## quotes.toscrape.com

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 15 | 0 | 2 | 4 | 51+ | 0 | 2.0 | 2.8 | 50 |
| webclaw-query | 15 | 3 | 9 | 11 | 20 | 11 | 1.3 | 3.0 | 50 |
| scrapling-bfs | 15 | 0 | 1 | 3 | 51+ | 0 | 1.8 | 2.8 | 50 |
| crawl4ai-bfs | 15 | 0 | 1 | 3 | 51+ | 0 | 7.4 | 13.8 | 49 |
| crawl4ai-best-first | 15 | 3 | 6 | 10 | 32 | 10 | 9.5 | 14.2 | 50 |
| crawl4ai-adaptive | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 2.5 | 3 |

crawl4ai-adaptive also requested the `<head>` of up to 90 linked pages per question (median) to preview them, on top of the pages above.
