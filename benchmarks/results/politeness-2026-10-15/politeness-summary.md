# Crawl politeness benchmark politeness-2026-10-15

Each arm crawls a local site one level deep (concurrency 4, robots.txt obeyed), set for the spacing in the scenario. The site answers each page after 100 ms and logs when each request arrives. `req/s` is page requests to the host divided by the time from the first to the last; the spacing allows at most 1/spacing. `too close` counts consecutive requests to the host that arrived less than 90% of the spacing apart. `seconds` is the whole run, process start included. Medians over rounds.

| scenario | spacing | arm | rounds | pages ok | requests | req/s | shortest gap ms | median gap ms | too close | seconds |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| delay 500 ms | 0.5 s | webclaw before (`--delay`) | 3 | 30 | 30 | 5.90 | 0 | 2 | 21/29 | 5.7 |
| delay 500 ms | 0.5 s | webclaw (`--delay`) | 3 | 30 | 30 | 1.99 | 500 | 502 | 0/29 | 15.4 |
| delay 500 ms | 0.5 s | Scrapling spider (`download_delay`) | 3 | 30 | 30 | 5.89 | 0 | 10 | 21/29 | 5.9 |
| delay 500 ms | 0.5 s | Scrapling spider (`download_delay`, `concurrent_requests_per_domain=1`) | 3 | 30 | 30 | 1.64 | 605 | 612 | 0/29 | 18.7 |
| delay 500 ms | 0.5 s | Crawl4AI BFS deep crawl (`mean_delay`, `max_range=0`) | 3 | 30 | 30 | 7.49 | 0 | 1 | 22/29 | 4.7 |
| delay 200 ms, Crawl-delay 1 | 1.0 s | webclaw before (`--delay`) | 3 | 12 | 12 | 1.00 | 998 | 1000 | 0/11 | 11.5 |
| delay 200 ms, Crawl-delay 1 | 1.0 s | webclaw (`--delay`) | 3 | 12 | 12 | 1.00 | 999 | 1002 | 0/11 | 12.3 |
| delay 200 ms, Crawl-delay 1 | 1.0 s | Scrapling spider (`download_delay`) | 3 | 12 | 12 | 3.26 | 0 | 1 | 8/11 | 4.8 |
| delay 200 ms, Crawl-delay 1 | 1.0 s | Scrapling spider (`download_delay`, `concurrent_requests_per_domain=1`) | 3 | 12 | 12 | 0.90 | 1106 | 1112 | 0/11 | 13.7 |
| delay 200 ms, Crawl-delay 1 | 1.0 s | Crawl4AI BFS deep crawl (`mean_delay`, `max_range=0`) | 3 | 12 | 12 | 8.37 | 0 | 198 | 11/11 | 2.1 |
