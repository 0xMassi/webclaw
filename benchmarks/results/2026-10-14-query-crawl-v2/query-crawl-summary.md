# Question-guided crawl benchmark 2026-10-14-query-crawl-v2

Each question names the page that answers it. Every arm crawls from the site's start page with a budget of 50 pages, depth 5, concurrency 4, the same path prefix, robots.txt obeyed. `answer by page N` counts questions whose answer page was among the first N pages the tool fetched. `median pages to answer` counts a miss as 51. `answer in output top 3` counts questions whose answer page is in the first 3 pages of the tool's own ranking (breadth-first arms: crawl order). `median seconds to answer` covers found answers only.

Question sets: `tuned` is the set the scoring weights were chosen on, `held-out` was written after they were fixed, on four other sites, and `intl` asks in Japanese, French, Spanish and Chinese.

## tuned: all sites

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 60 | 0 | 2 | 4 | 51+ | 0 | 1.9 | 2.4 | 50 |
| webclaw-query | 60 | 31 | 50 | 54 | 10 | 52 | 0.6 | 1.2 | 50 |
| scrapling-bfs | 60 | 0 | 1 | 3 | 51+ | 0 | 1.7 | 1.5 | 50 |
| crawl4ai-bfs | 60 | 0 | 1 | 3 | 51+ | 0 | 8.1 | 10.5 | 49 |
| crawl4ai-best-first | 60 | 18 | 37 | 46 | 14 | 33 | 4.4 | 9.8 | 50 |
| crawl4ai-adaptive | 60 | 2 | 3 | 3 | 51+ | 3 | 2.3 | 1.8 | 7 |

### tuned: docs.python.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 3.5 | 50 |
| webclaw-query | 15 | 9 | 13 | 14 | 9 | 13 | 0.3 | 1.1 | 50 |
| scrapling-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 1.5 | 50 |
| crawl4ai-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 10.6 | 49 |
| crawl4ai-best-first | 15 | 0 | 9 | 11 | 14 | 7 | 4.3 | 10.0 | 50 |
| crawl4ai-adaptive | 15 | 0 | 1 | 1 | 51+ | 1 | 2.6 | 3.2 | 19 |

### tuned: www.postgresql.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 2.2 | 50 |
| webclaw-query | 15 | 5 | 13 | 14 | 13 | 14 | 0.6 | 1.2 | 50 |
| scrapling-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 1.2 | 50 |
| crawl4ai-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 9.0 | 49 |
| crawl4ai-best-first | 15 | 0 | 7 | 10 | 28 | 4 | 5.2 | 9.0 | 50 |
| crawl4ai-adaptive | 15 | 2 | 2 | 2 | 51+ | 2 | 2.1 | 1.6 | 5 |

### tuned: developer.mozilla.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 1.5 | 50 |
| webclaw-query | 15 | 14 | 15 | 15 | 5 | 14 | 0.2 | 0.9 | 50 |
| scrapling-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 1.5 | 50 |
| crawl4ai-bfs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 10.4 | 49 |
| crawl4ai-best-first | 15 | 15 | 15 | 15 | 2 | 12 | 2.6 | 9.5 | 50 |
| crawl4ai-adaptive | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 1.1 | 2 |

### tuned: quotes.toscrape.com

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 15 | 0 | 2 | 4 | 51+ | 0 | 1.9 | 2.7 | 50 |
| webclaw-query | 15 | 3 | 9 | 11 | 19 | 11 | 1.3 | 2.9 | 50 |
| scrapling-bfs | 15 | 0 | 1 | 3 | 51+ | 0 | 1.7 | 2.7 | 50 |
| crawl4ai-bfs | 15 | 0 | 1 | 3 | 51+ | 0 | 8.1 | 14.2 | 49 |
| crawl4ai-best-first | 15 | 3 | 6 | 10 | 32 | 10 | 9.5 | 14.3 | 50 |
| crawl4ai-adaptive | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 2.5 | 3 |

## held-out: all sites

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 48 | 4 | 12 | 21 | 51+ | 0 | 1.7 | 2.6 | 50 |
| webclaw-query | 48 | 33 | 38 | 43 | 6 | 36 | 0.8 | 3.7 | 50 |
| scrapling-bfs | 48 | 4 | 12 | 21 | 51+ | 0 | 2.0 | 3.2 | 50 |
| crawl4ai-bfs | 48 | 0 | 4 | 17 | 51+ | 0 | 9.1 | 14.5 | 49 |
| crawl4ai-best-first | 48 | 17 | 21 | 27 | 29 | 13 | 4.5 | 14.3 | 50 |
| crawl4ai-adaptive | 48 | 5 | 5 | 5 | 51+ | 5 | 2.6 | 2.1 | 6 |

### held-out: kubernetes.io

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 12 | 1 | 1 | 3 | 51+ | 0 | 0.8 | 1.2 | 50 |
| webclaw-query | 12 | 7 | 9 | 11 | 6 | 10 | 0.4 | 2.0 | 50 |
| scrapling-bfs | 12 | 1 | 1 | 3 | 51+ | 0 | 1.4 | 2.0 | 50 |
| crawl4ai-bfs | 12 | 0 | 1 | 3 | 51+ | 0 | 9.0 | 15.9 | 49 |
| crawl4ai-best-first | 12 | 5 | 5 | 6 | 40 | 4 | 4.8 | 15.8 | 50 |
| crawl4ai-adaptive | 12 | 0 | 0 | 0 | 51+ | 0 | nan | 2.0 | 3 |

### held-out: docs.djangoproject.com

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 12 | 0 | 0 | 2 | 51+ | 0 | 1.0 | 1.2 | 50 |
| webclaw-query | 12 | 9 | 11 | 12 | 7 | 10 | 0.3 | 0.8 | 50 |
| scrapling-bfs | 12 | 0 | 0 | 2 | 51+ | 0 | 0.6 | 0.8 | 50 |
| crawl4ai-bfs | 12 | 0 | 0 | 2 | 51+ | 0 | 8.2 | 9.7 | 49 |
| crawl4ai-best-first | 12 | 6 | 7 | 7 | 10 | 3 | 2.9 | 8.4 | 50 |
| crawl4ai-adaptive | 12 | 1 | 1 | 1 | 51+ | 1 | 2.6 | 1.8 | 7 |

### held-out: sqlite.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 12 | 0 | 3 | 5 | 51+ | 0 | 3.6 | 6.3 | 50 |
| webclaw-query | 12 | 8 | 8 | 9 | 8 | 7 | 2.2 | 6.1 | 50 |
| scrapling-bfs | 12 | 0 | 3 | 5 | 51+ | 0 | 2.9 | 5.1 | 50 |
| crawl4ai-bfs | 12 | 0 | 2 | 5 | 51+ | 0 | 8.2 | 13.1 | 49 |
| crawl4ai-best-first | 12 | 3 | 4 | 5 | 51+ | 3 | 4.4 | 13.7 | 50 |
| crawl4ai-adaptive | 12 | 1 | 1 | 1 | 51+ | 1 | 2.3 | 9.8 | 5 |

### held-out: go.dev

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 12 | 3 | 8 | 11 | 16 | 0 | 1.7 | 4.0 | 50 |
| webclaw-query | 12 | 9 | 10 | 11 | 4 | 9 | 0.9 | 4.5 | 50 |
| scrapling-bfs | 12 | 3 | 8 | 11 | 18 | 0 | 2.0 | 4.4 | 50 |
| crawl4ai-bfs | 12 | 0 | 1 | 7 | 40 | 0 | 10.1 | 17.1 | 49 |
| crawl4ai-best-first | 12 | 3 | 5 | 9 | 28 | 3 | 8.1 | 18.4 | 50 |
| crawl4ai-adaptive | 12 | 3 | 3 | 3 | 51+ | 3 | 3.3 | 3.2 | 10 |

## intl: all sites

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 16 | 0 | 0 | 0 | 51+ | 0 | nan | 3.7 | 50 |
| webclaw-query | 16 | 3 | 12 | 15 | 18 | 10 | 0.6 | 1.6 | 50 |
| scrapling-bfs | 16 | 0 | 0 | 1 | 51+ | 0 | 2.8 | 3.3 | 50 |
| crawl4ai-bfs | 16 | 0 | 0 | 0 | 51+ | 0 | nan | 10.5 | 49 |
| crawl4ai-best-first | 16 | 0 | 2 | 2 | 51+ | 2 | 3.9 | 10.0 | 50 |
| crawl4ai-adaptive | 16 | 2 | 2 | 2 | 51+ | 1 | 4.3 | 5.3 | 14 |

### intl: py-ja

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 6.8 | 50 |
| webclaw-query | 5 | 1 | 3 | 5 | 25 | 3 | 0.6 | 1.4 | 50 |
| scrapling-bfs | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 3.3 | 50 |
| crawl4ai-bfs | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 10.0 | 49 |
| crawl4ai-best-first | 5 | 0 | 1 | 1 | 51+ | 1 | 3.9 | 9.7 | 50 |
| crawl4ai-adaptive | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 1.9 | 7 |

### intl: py-fr

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 1.6 | 50 |
| webclaw-query | 5 | 2 | 4 | 5 | 11 | 4 | 0.4 | 1.5 | 50 |
| scrapling-bfs | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 1.0 | 50 |
| crawl4ai-bfs | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 10.7 | 49 |
| crawl4ai-best-first | 5 | 0 | 1 | 1 | 51+ | 1 | 4.0 | 9.9 | 50 |
| crawl4ai-adaptive | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 6.1 | 17 |

### intl: py-es

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 2.0 | 50 |
| webclaw-query | 3 | 0 | 2 | 2 | 20 | 0 | 0.7 | 1.8 | 50 |
| scrapling-bfs | 3 | 0 | 0 | 1 | 51+ | 0 | 2.8 | 3.2 | 50 |
| crawl4ai-bfs | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 10.3 | 49 |
| crawl4ai-best-first | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 10.1 | 50 |
| crawl4ai-adaptive | 3 | 2 | 2 | 2 | 6 | 1 | 4.3 | 13.9 | 29 |

### intl: py-zh

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-bfs | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 5.4 | 50 |
| webclaw-query | 3 | 0 | 3 | 3 | 16 | 3 | 0.6 | 1.8 | 50 |
| scrapling-bfs | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 3.5 | 50 |
| crawl4ai-bfs | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 10.9 | 49 |
| crawl4ai-best-first | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 10.0 | 50 |
| crawl4ai-adaptive | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 2.8 | 7 |

crawl4ai-adaptive also requested the `<head>` of up to 145 linked pages per question (median) to preview them, on top of the pages above.
