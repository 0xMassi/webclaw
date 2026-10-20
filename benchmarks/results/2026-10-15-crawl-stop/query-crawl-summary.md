# Question-guided crawl benchmark 2026-10-15-crawl-stop

Each question names the page that answers it. Every arm crawls from the site's start page with a budget of 50 pages, depth 5, concurrency 4, the same path prefix, robots.txt obeyed. `answer by page N` counts questions whose answer page was among the first N pages the tool fetched. `median pages to answer` counts a miss as 51. `answer in output top 3` counts questions whose answer page is in the first 3 pages of the tool's own ranking (breadth-first arms: crawl order). `median seconds to answer` covers found answers only.

Question sets: `tuned` is the set the scoring weights were chosen on, `held-out` was written after they were fixed, on four other sites, `intl` asks in Japanese, French, Spanish and Chinese, and `fresh` was written on four more sites after the stop rule of `--stop-when-answered` was fixed.

## tuned: all sites

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 60 | 29 | 48 | 52 | 11 | 50 | 0.2 | 1.0 | 50 |
| webclaw-query-stop | 60 | 28 | 51 | 52 | 12 | 50 | 0.2 | 0.4 | 32 |
| crawl4ai-adaptive | 60 | 0 | 0 | 0 | 51+ | 0 | nan | 0.6 | 7 |
| crawl4ai-adaptive-docs | 60 | 0 | 1 | 1 | 51+ | 1 | 2.4 | 0.7 | 9 |

### tuned: docs.python.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 15 | 8 | 13 | 14 | 10 | 13 | 0.2 | 1.1 | 50 |
| webclaw-query-stop | 15 | 7 | 13 | 13 | 11 | 13 | 0.1 | 0.4 | 27 |
| crawl4ai-adaptive | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 1.0 | 13 |
| crawl4ai-adaptive-docs | 15 | 0 | 1 | 1 | 51+ | 1 | 2.4 | 2.1 | 19 |

### tuned: www.postgresql.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 15 | 5 | 12 | 13 | 14 | 12 | 0.4 | 0.6 | 50 |
| webclaw-query-stop | 15 | 3 | 14 | 14 | 13 | 13 | 0.2 | 0.4 | 38 |
| crawl4ai-adaptive | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 0.6 | 10 |
| crawl4ai-adaptive-docs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 0.6 | 6 |

### tuned: developer.mozilla.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 15 | 13 | 15 | 15 | 4 | 15 | 0.2 | 1.1 | 50 |
| webclaw-query-stop | 15 | 15 | 15 | 15 | 3 | 14 | 0.1 | 0.1 | 29 |
| crawl4ai-adaptive | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 0.5 | 2 |
| crawl4ai-adaptive-docs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 0.7 | 6 |

### tuned: quotes.toscrape.com

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 15 | 3 | 8 | 10 | 23 | 10 | 0.7 | 1.3 | 50 |
| webclaw-query-stop | 15 | 3 | 9 | 10 | 20 | 10 | 0.7 | 0.7 | 34 |
| crawl4ai-adaptive | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 0.6 | 3 |
| crawl4ai-adaptive-docs | 15 | 0 | 0 | 0 | 51+ | 0 | nan | 0.5 | 6 |

## held-out: all sites

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 48 | 30 | 35 | 41 | 8 | 37 | 0.3 | 0.9 | 50 |
| webclaw-query-stop | 48 | 29 | 36 | 40 | 8 | 36 | 0.3 | 0.4 | 39 |
| crawl4ai-adaptive | 48 | 3 | 3 | 3 | 51+ | 3 | 1.5 | 0.7 | 6 |
| crawl4ai-adaptive-docs | 48 | 6 | 7 | 7 | 51+ | 7 | 2.1 | 0.9 | 11 |

### held-out: kubernetes.io

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 12 | 6 | 8 | 11 | 10 | 11 | 0.2 | 1.0 | 50 |
| webclaw-query-stop | 12 | 7 | 8 | 11 | 8 | 10 | 0.2 | 0.4 | 39 |
| crawl4ai-adaptive | 12 | 0 | 0 | 0 | 51+ | 0 | nan | 0.6 | 3 |
| crawl4ai-adaptive-docs | 12 | 0 | 0 | 0 | 51+ | 0 | nan | 0.7 | 6 |

### held-out: docs.djangoproject.com

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 12 | 7 | 10 | 11 | 8 | 10 | 0.1 | 0.4 | 50 |
| webclaw-query-stop | 12 | 7 | 10 | 11 | 8 | 10 | 0.1 | 0.3 | 39 |
| crawl4ai-adaptive | 12 | 1 | 1 | 1 | 51+ | 1 | 1.3 | 1.1 | 12 |
| crawl4ai-adaptive-docs | 12 | 1 | 1 | 1 | 51+ | 1 | 0.5 | 0.9 | 11 |

### held-out: sqlite.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 12 | 8 | 8 | 9 | 8 | 8 | 0.8 | 3.6 | 50 |
| webclaw-query-stop | 12 | 7 | 8 | 8 | 8 | 8 | 0.7 | 1.2 | 40 |
| crawl4ai-adaptive | 12 | 0 | 0 | 0 | 51+ | 0 | nan | 0.7 | 5 |
| crawl4ai-adaptive-docs | 12 | 2 | 2 | 2 | 51+ | 2 | 3.6 | 0.7 | 11 |

### held-out: go.dev

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 12 | 9 | 9 | 10 | 5 | 8 | 0.3 | 0.7 | 50 |
| webclaw-query-stop | 12 | 8 | 10 | 10 | 6 | 8 | 0.3 | 0.4 | 39 |
| crawl4ai-adaptive | 12 | 2 | 2 | 2 | 51+ | 2 | 3.2 | 1.0 | 5 |
| crawl4ai-adaptive-docs | 12 | 3 | 4 | 4 | 51+ | 4 | 1.3 | 2.7 | 17 |

## intl: all sites

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 16 | 4 | 12 | 12 | 16 | 9 | 0.3 | 1.5 | 50 |
| webclaw-query-stop | 16 | 3 | 12 | 13 | 16 | 9 | 0.1 | 0.5 | 48 |
| crawl4ai-adaptive | 16 | 1 | 2 | 2 | 51+ | 2 | 1.4 | 2.1 | 14 |
| crawl4ai-adaptive-docs | 16 | 1 | 2 | 2 | 51+ | 2 | 1.6 | 2.0 | 18 |

### intl: py-ja

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 5 | 1 | 3 | 3 | 14 | 2 | 0.3 | 1.6 | 50 |
| webclaw-query-stop | 5 | 1 | 2 | 3 | 29 | 2 | 0.1 | 0.5 | 50 |
| crawl4ai-adaptive | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 0.7 | 7 |
| crawl4ai-adaptive-docs | 5 | 0 | 0 | 0 | 51+ | 0 | nan | 0.5 | 6 |

### intl: py-fr

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 5 | 1 | 4 | 4 | 21 | 3 | 0.3 | 1.2 | 50 |
| webclaw-query-stop | 5 | 2 | 5 | 5 | 13 | 4 | 0.1 | 0.5 | 44 |
| crawl4ai-adaptive | 5 | 0 | 1 | 1 | 51+ | 1 | 0.9 | 3.0 | 25 |
| crawl4ai-adaptive-docs | 5 | 0 | 1 | 1 | 51+ | 1 | 1.4 | 4.9 | 31 |

### intl: py-es

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 3 | 1 | 2 | 2 | 15 | 1 | 0.3 | 1.6 | 50 |
| webclaw-query-stop | 3 | 0 | 2 | 2 | 15 | 0 | 0.1 | 0.6 | 50 |
| crawl4ai-adaptive | 3 | 1 | 1 | 1 | 51+ | 1 | 1.9 | 1.8 | 16 |
| crawl4ai-adaptive-docs | 3 | 1 | 1 | 1 | 51+ | 1 | 1.8 | 2.0 | 21 |

### intl: py-zh

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 3 | 1 | 3 | 3 | 13 | 3 | 0.5 | 1.4 | 50 |
| webclaw-query-stop | 3 | 0 | 3 | 3 | 18 | 3 | 0.2 | 0.5 | 50 |
| crawl4ai-adaptive | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 1.0 | 7 |
| crawl4ai-adaptive-docs | 3 | 0 | 0 | 0 | 51+ | 0 | nan | 1.3 | 11 |

## fresh: all sites

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 48 | 22 | 36 | 40 | 11 | 27 | 0.2 | 0.6 | 50 |
| webclaw-query-stop | 48 | 24 | 38 | 40 | 10 | 27 | 0.1 | 0.4 | 44 |
| crawl4ai-adaptive | 48 | 4 | 6 | 6 | 51+ | 3 | 0.7 | 0.9 | 13 |
| crawl4ai-adaptive-docs | 48 | 5 | 8 | 8 | 51+ | 6 | 0.6 | 0.7 | 14 |

### fresh: nginx.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 12 | 8 | 11 | 12 | 6 | 7 | 0.2 | 0.3 | 50 |
| webclaw-query-stop | 12 | 9 | 12 | 12 | 6 | 7 | 0.1 | 0.2 | 41 |
| crawl4ai-adaptive | 12 | 1 | 1 | 1 | 51+ | 1 | 0.6 | 0.9 | 13 |
| crawl4ai-adaptive-docs | 12 | 1 | 1 | 1 | 51+ | 1 | 0.6 | 0.6 | 6 |

### fresh: nodejs.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 12 | 5 | 11 | 12 | 11 | 10 | 0.3 | 0.6 | 50 |
| webclaw-query-stop | 12 | 6 | 11 | 12 | 10 | 10 | 0.2 | 0.5 | 42 |
| crawl4ai-adaptive | 12 | 3 | 5 | 5 | 51+ | 2 | 0.8 | 27.2 | 27 |
| crawl4ai-adaptive-docs | 12 | 3 | 6 | 6 | 38 | 5 | 1.2 | 26.6 | 38 |

### fresh: doc.rust-lang.org

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 12 | 6 | 10 | 11 | 10 | 7 | 0.2 | 0.9 | 50 |
| webclaw-query-stop | 12 | 7 | 10 | 11 | 8 | 7 | 0.1 | 0.4 | 47 |
| crawl4ai-adaptive | 12 | 0 | 0 | 0 | 51+ | 0 | nan | 0.8 | 6 |
| crawl4ai-adaptive-docs | 12 | 1 | 1 | 1 | 51+ | 0 | 0.6 | 0.7 | 6 |

### fresh: git-scm.com

| arm | questions | answer by page 10 | by 25 | by 50 | median pages to answer | answer in output top 3 | median seconds to answer | median seconds per crawl | pages fetched |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-query | 12 | 3 | 4 | 5 | 51+ | 3 | 0.2 | 0.7 | 50 |
| webclaw-query-stop | 12 | 2 | 5 | 5 | 51+ | 3 | 0.1 | 0.4 | 48 |
| crawl4ai-adaptive | 12 | 0 | 0 | 0 | 51+ | 0 | nan | 0.9 | 7 |
| crawl4ai-adaptive-docs | 12 | 0 | 0 | 0 | 51+ | 0 | nan | 0.8 | 6 |

crawl4ai-adaptive also requested the `<head>` of up to 197 linked pages per question (median) to preview them, on top of the pages above.

crawl4ai-adaptive-docs also requested the `<head>` of up to 270 linked pages per question (median) to preview them, on top of the pages above.
