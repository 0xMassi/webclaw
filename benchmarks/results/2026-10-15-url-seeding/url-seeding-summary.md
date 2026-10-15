# URL discovery 2026-10-15-url-seeding

Each arm lists the URLs of 10 sites once, starting from `https://<site>/`. `in scope` counts distinct URLs on the site's own host (`www.` ignored), compared without scheme, fragment or trailing slash. `live` is the share of a seeded sample of up to 20 in-scope URLs per arm that answered 200 with HTML after redirects, fetched with curl_cffi impersonating Chrome, one request a second across sites taken in turn; answers that refuse the checker (403, 419, 429, 503 or no answer) are fetched again ten seconds apart, and those still refused are left out of the share. `est. live` is in scope times live. `of union` is the arm's share of every in-scope URL any arm found on that site. `dups` counts URLs returned more than once (same comparison). `with a query string` counts in-scope URLs with a `?` part, which in the index are often the same page with a tracking or search parameter. Times are wall clock for one cold run; the Common Crawl index is a shared public service and its answer times vary from run to run.

Totals over all sites:

| arm | URLs | dups | in scope | with a query string | live (sampled) | est. live | of union (median) | seconds (median) | errors |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 16,659 | 0 | 16,659 | 1,252 | 94% (161) | 15,674 | 80% | 8.3 | 1 |
| webclaw-sitemap-only | 10,084 | 0 | 10,084 | 796 | 94% (87) | 9,450 | 0% | 1.4 | 0 |
| webclaw-cc | 17,828 | 1 | 17,827 | 6,174 | 100% (162) | 17,827 | 34% | 14.7 | 1 |
| webclaw-sitemap+cc | 36,653 | 1 | 36,652 | 6,620 | 96% (200) | 35,870 | 77% | 12.1 | 1 |
| crawl4ai-sitemap | 9,782 | 0 | 9,782 | 0 | 100% (86) | 9,782 | 0% | 5.5 | 0 |
| crawl4ai-cc | 33,123 | 195 | 28,653 | 6,525 | 96% (143) | 28,489 | 35% | 14.4 | 2 |
| crawl4ai-sitemap+cc | 16,058 | 87 | 15,912 | 235 | 96% (139) | 15,500 | 26% | 15.7 | 7 |
| scrapling-sitemap | 7,548 | 1 | 7,547 | 0 | 100% (67) | 7,547 | 0% | 0.7 | 0 |

In-scope URLs per site (live share of the sample in brackets):

| site | kind | webclaw | webclaw-sitemap-only | webclaw-cc | webclaw-sitemap+cc | crawl4ai-sitemap | crawl4ai-cc | crawl4ai-sitemap+cc | scrapling-sitemap |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| docs.python.org | docs, a 7-URL sitemap | 3,357 (100%) | 7 (100%) | 6,297 (100%) | 6,302 (100%) | 6 (100%) | 6,944 (100%) | 984 (85%) E | 7 (100%) |
| www.sqlite.org | docs, no sitemap | 1,684 (100%) | 0 (-) | 426 (100%) | 426 (100%) | 0 (-) | 429 (95%) | 0 (-) E | 0 (-) |
| go.dev | docs, no sitemap | 883 (90%) | 0 (-) | 2,696 (100%) | 2,696 (100%) | 0 (-) | 0 (-) E | 0 (-) E | 0 (-) |
| kubernetes.io | docs, a full sitemap | 6,620 (100%) | 6,620 (100%) | 2,615 (100%) | 6,619 (100%) E | 6,331 (100%) | 2,622 (95%) | 6,331 (100%) E | 6,620 (100%) |
| news.ycombinator.com | news, no sitemap | 0 (-) E | 0 (-) | 5,375 (100%) | 5,375 (100%) | 0 (-) | 5,923 (100%) | 0 (-) E | 0 (-) |
| lwn.net | news, no sitemap | 1 (100%) | 0 (-) | 0 (-) E | 11,392 (100%) | 0 (-) | 12,583 (100%) | 4,936 (100%) E | 0 (-) |
| books.toscrape.com | shop, no sitemap | 629 (100%) | 0 (-) | 288 (100%) | 288 (100%) | 0 (-) | 0 (-) E | 134 (100%) E | 0 (-) |
| www.allbirds.com | shop, a full sitemap | 2,537 (65%) | 2,537 (75%) | 2 (100%) | 2,538 (70%) | 2,533 (100%) | 4 (80%) | 2,536 (90%) | 0 (-) |
| danluu.com | blog, a full sitemap | 180 (95%) | 152 (100%) | 66 (100%) | 212 (90%) | 152 (100%) | 75 (90%) | 193 (95%) | 152 (100%) |
| jvns.ca | blog, a full sitemap | 768 (100%) | 768 (100%) | 62 (100%) | 804 (100%) | 760 (100%) | 73 (95%) | 798 (100%) | 768 (100%) |

`E`: the arm reported an error (below). Seconds per site:

| site | webclaw | webclaw-sitemap-only | webclaw-cc | webclaw-sitemap+cc | crawl4ai-sitemap | crawl4ai-cc | crawl4ai-sitemap+cc | scrapling-sitemap |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| docs.python.org | 8.5 | 0.2 | 17.9 | 11.9 | 5.5 | 26.9 | 16.9 | 0.3 |
| www.sqlite.org | 18.6 | 3.8 | 13.7 | 7.8 | 5.4 | 9.2 | 15.7 | 3.4 |
| go.dev | 14.0 | 1.6 | 8.9 | 12.0 | 5.3 | 10.7 | 15.7 | 0.5 |
| kubernetes.io | 5.0 | 0.8 | 7.9 | 62.5 | 6.0 | 19.8 | 17.0 | 1.9 |
| news.ycombinator.com | 900.0 | 2.1 | 14.7 | 12.1 | 5.8 | 14.4 | 10.9 | 0.9 |
| lwn.net | 2.2 | 1.3 | 46.6 | 51.1 | 5.5 | 24.4 | 19.1 | 1.0 |
| books.toscrape.com | 8.3 | 1.5 | 12.4 | 8.6 | 5.4 | 15.7 | 10.7 | 0.5 |
| www.allbirds.com | 1.3 | 1.4 | 20.9 | 6.6 | 5.9 | 6.4 | 15.4 | 0.3 |
| danluu.com | 7.7 | 0.3 | 15.8 | 21.4 | 5.4 | 7.0 | 6.7 | 0.7 |
| jvns.ca | 0.5 | 0.6 | 9.0 | 21.0 | 5.5 | 12.7 | 9.3 | 0.4 |

Errors:

- docs.python.org crawl4ai-sitemap+cc: JSONDecodeError: Unterminated string starting at: line 1 column 336 (char 335)
- www.sqlite.org crawl4ai-sitemap+cc: HTTPStatusError: Server error '502 Bad Gateway' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.www.sqlite.org%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/502
- go.dev crawl4ai-cc: HTTPStatusError: Server error '502 Bad Gateway' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.go.dev%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/502
- go.dev crawl4ai-sitemap+cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.go.dev%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504
- kubernetes.io webclaw-sitemap+cc: warning: the Common Crawl index could not be read (response body decode failed: error decoding response body: error reading a body from connection); it is often busy, try again later or raise --timeout
- kubernetes.io crawl4ai-sitemap+cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.kubernetes.io%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504
- news.ycombinator.com webclaw: no answer in 900s
- news.ycombinator.com crawl4ai-sitemap+cc: HTTPStatusError: Server error '502 Bad Gateway' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.news.ycombinator.com%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/502
- lwn.net webclaw-cc: error: the Common Crawl index could not be read (the target returned HTTP 504 instead of a successful page); it is often busy, try again later or raise --timeout
- lwn.net crawl4ai-sitemap+cc: JSONDecodeError: Unterminated string starting at: line 1 column 53 (char 52)
- books.toscrape.com crawl4ai-cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.books.toscrape.com%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504
- books.toscrape.com crawl4ai-sitemap+cc: JSONDecodeError: Unterminated string starting at: line 1 column 116 (char 115)

Rank of the page that answers a question in `webclaw --map --map-source sitemap,cc`, without and with `--query` (Crawl4AI ranks by a query only after fetching the `<head>` of every URL, so it has no row here):

| site | question | target | URLs | rank without | rank with --query |
|---|---|---|---:|---:|---:|
| docs.python.org | how do I cancel an asyncio task | docs.python.org/3/library/asyncio-task.html | 6,303 | 575 | 1 |
| www.sqlite.org | write-ahead log mode | sqlite.org/wal.html | 426 | absent | absent |
| go.dev | tutorial on generics | go.dev/doc/tutorial/generics | 2,696 | absent | absent |
| kubernetes.io | configure liveness probes | kubernetes.io/docs/tasks/configure-pod-container/configure-liveness-readiness-startup-probes | 7,342 | 783 | 1 |
