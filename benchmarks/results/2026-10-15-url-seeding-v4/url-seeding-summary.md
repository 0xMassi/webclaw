# URL discovery 2026-10-15-url-seeding-v4

Each arm lists the URLs of 10 sites once, starting from `https://<site>/`. `in scope` counts distinct URLs on the site's own host (`www.` ignored), compared without scheme, fragment or trailing slash. `live` is the share of a seeded sample of up to 20 in-scope URLs per arm that answered 200 with HTML after redirects, fetched with curl_cffi impersonating Chrome, one request a second across sites taken in turn; answers that refuse the checker (403, 419, 429, 503 or no answer) are fetched again ten seconds apart, and those still refused are left out of the share. `est. live` is in scope times live. `of union` is the arm's share of every in-scope URL any arm found on that site. `dups` counts URLs returned more than once (same comparison). `with a query string` counts in-scope URLs with a `?` part, which in the index are often the same page with a tracking or search parameter. Times are wall clock for one cold run; the Common Crawl index is a shared public service and its answer times vary from run to run, so the index arms of both tools run in one stretch, site by site, alternating which tool goes first. An arm that gives no answer in 300 s is stopped.

Totals over all sites:

| arm | URLs | dups | in scope | with a query string | live (sampled) | est. live | of union (median) | seconds (median) | errors |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 17,026 | 0 | 17,026 | 1,251 | 96% (161) | 16,138 | 80% | 7.3 | 1 |
| webclaw-sitemap-only | 10,083 | 0 | 10,083 | 796 | 94% (87) | 9,449 | 0% | 1.4 | 0 |
| webclaw-cc | 11,502 | 1 | 11,501 | 322 | 100% (81) | 11,501 | 0% | 23.1 | 5 |
| webclaw-sitemap+cc | 44,125 | 3 | 44,122 | 1,451 | 96% (175) | 43,352 | 97% | 41.5 | 4 |
| crawl4ai-sitemap | 9,781 | 0 | 9,781 | 0 | 100% (86) | 9,781 | 0% | 5.6 | 0 |
| crawl4ai-cc | 14,349 | 138 | 9,995 | 551 | 97% (60) | 9,843 | 0% | 15.7 | 7 |
| crawl4ai-sitemap+cc | 29,472 | 122 | 29,008 | 6,031 | 99% (161) | 28,754 | 76% | 16.1 | 5 |
| scrapling-sitemap | 7,547 | 1 | 7,546 | 0 | 100% (67) | 7,546 | 0% | 0.5 | 0 |

In-scope URLs per site (live share of the sample in brackets):

| site | kind | webclaw | webclaw-sitemap-only | webclaw-cc | webclaw-sitemap+cc | crawl4ai-sitemap | crawl4ai-cc | crawl4ai-sitemap+cc | scrapling-sitemap |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| docs.python.org | docs, a 7-URL sitemap | 3,589 (100%) | 7 (100%) | 6,222 (100%) | 8,560 (100%) | 6 (100%) | 6,944 (100%) | 6 (100%) E | 7 (100%) |
| www.sqlite.org | docs, no sitemap | 1,683 (100%) | 0 (-) | 0 (-) E | 1,959 (100%) | 0 (-) | 429 (95%) | 429 (100%) | 0 (-) |
| go.dev | docs, no sitemap | 1,029 (100%) | 0 (-) | 2,683 (100%) | 3,600 (100%) | 0 (-) | 0 (-) E | 0 (-) E | 0 (-) |
| kubernetes.io | docs, a full sitemap | 6,619 (100%) | 6,619 (100%) | 2,533 (100%) | 6,619 (100%) E | 6,330 (100%) | 2,622 (95%) | 6,330 (100%) E | 6,619 (100%) |
| news.ycombinator.com | news, no sitemap | 0 (-) E | 0 (-) | 0 (-) E | 0 (-) E | 0 (-) | 0 (-) E | 5,923 (100%) | 0 (-) |
| lwn.net | news, no sitemap | 1 (100%) | 0 (-) | 0 (-) E | 19,263 (100%) | 0 (-) | 0 (-) E | 12,583 (100%) | 0 (-) |
| books.toscrape.com | shop, no sitemap | 629 (100%) | 0 (-) | 0 (-) E | 629 (100%) E | 0 (-) | 0 (-) E | 289 (100%) | 0 (-) |
| www.allbirds.com | shop, a full sitemap | 2,537 (65%) | 2,537 (75%) | 1 (100%) | 2,537 (70%) | 2,533 (100%) | 0 (-) E | 2,536 (90%) | 0 (-) |
| danluu.com | blog, a full sitemap | 171 (100%) | 152 (100%) | 62 (100%) | 187 (95%) | 152 (100%) | 0 (-) E | 152 (100%) E | 152 (100%) |
| jvns.ca | blog, a full sitemap | 768 (100%) | 768 (100%) | 0 (-) E | 768 (100%) E | 760 (100%) | 0 (-) E | 760 (100%) E | 768 (100%) |

`E`: the arm reported an error (below). Seconds per site:

| site | webclaw | webclaw-sitemap-only | webclaw-cc | webclaw-sitemap+cc | crawl4ai-sitemap | crawl4ai-cc | crawl4ai-sitemap+cc | scrapling-sitemap |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| docs.python.org | 7.3 | 0.2 | 13.7 | 16.6 | 5.7 | 20.3 | 16.1 | 0.3 |
| www.sqlite.org | 16.8 | 3.8 | 32.5 | 29.1 | 5.5 | 8.5 | 17.1 | 3.3 |
| go.dev | 15.0 | 2.0 | 23.1 | 51.9 | 5.7 | 10.8 | 10.7 | 0.4 |
| kubernetes.io | 3.1 | 4.5 | 41.0 | 41.5 | 6.5 | 53.2 | 16.1 | 0.5 |
| news.ycombinator.com | 300.0 | 5.1 | 38.7 | 300.0 | 5.6 | 15.7 | 16.7 | 1.2 |
| lwn.net | 2.2 | 1.3 | 8.1 | 75.5 | 5.3 | 0.6 | 22.2 | 0.9 |
| books.toscrape.com | 8.2 | 1.4 | 8.0 | 67.1 | 5.4 | 0.9 | 12.0 | 0.5 |
| www.allbirds.com | 1.6 | 1.3 | 9.2 | 9.0 | 6.1 | 0.6 | 7.8 | 0.3 |
| danluu.com | 4.8 | 0.5 | 18.4 | 15.6 | 5.4 | 15.7 | 15.8 | 0.3 |
| jvns.ca | 0.4 | 0.4 | 43.9 | 38.9 | 5.3 | 15.7 | 16.4 | 0.5 |

Errors:

- docs.python.org crawl4ai-sitemap+cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.docs.python.org%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504
- www.sqlite.org webclaw-cc: error: the Common Crawl lookup stopped early (the index answered HTTP 502); the index is often busy, try again later
- go.dev crawl4ai-cc: HTTPStatusError: Server error '502 Bad Gateway' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.go.dev%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/502
- go.dev crawl4ai-sitemap+cc: HTTPStatusError: Server error '502 Bad Gateway' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.go.dev%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/502
- kubernetes.io webclaw-sitemap+cc: warning: the Common Crawl lookup stopped early (the index answered HTTP 504); the index is often busy, try again later; listing the URLs found until then
- kubernetes.io crawl4ai-sitemap+cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.kubernetes.io%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504
- news.ycombinator.com webclaw: no answer in 300s
- news.ycombinator.com webclaw-cc: error: the Common Crawl lookup stopped early (the index answered HTTP 504); the index is often busy, try again later
- news.ycombinator.com webclaw-sitemap+cc: no answer in 300s
- news.ycombinator.com crawl4ai-cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.news.ycombinator.com%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504
- lwn.net webclaw-cc: error: the Common Crawl lookup stopped early (request failed: error sending request for uri (https://index.commoncrawl.org/collinfo.json): client error (SendRequest)); the index is often busy, try again later
- lwn.net crawl4ai-cc: RemoteProtocolError: Server disconnected without sending a response.
- books.toscrape.com webclaw-cc: error: the Common Crawl lookup stopped early (request failed: error sending request for uri (https://index.commoncrawl.org/collinfo.json): client error (SendRequest)); the index is often busy, try again later
- books.toscrape.com webclaw-sitemap+cc: warning: the Common Crawl lookup stopped early (the index answered HTTP 504); the index is often busy, try again later; listing the URLs found until then
- books.toscrape.com crawl4ai-cc: RemoteProtocolError: Server disconnected without sending a response.
- www.allbirds.com crawl4ai-cc: RemoteProtocolError: Server disconnected without sending a response.
- danluu.com crawl4ai-cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.danluu.com%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504
- danluu.com crawl4ai-sitemap+cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.danluu.com%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504
- jvns.ca webclaw-cc: error: the Common Crawl lookup stopped early (the index answered HTTP 504); the index is often busy, try again later
- jvns.ca webclaw-sitemap+cc: warning: the Common Crawl lookup stopped early (the index answered HTTP 504); the index is often busy, try again later; listing the URLs found until then
- jvns.ca crawl4ai-cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.jvns.ca%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504
- jvns.ca crawl4ai-sitemap+cc: HTTPStatusError: Server error '504 Gateway Time-out' for url 'https://index.commoncrawl.org/CC-MAIN-2026-39-index?url=*.jvns.ca%2F*&output=json'
For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/504

Rank of the page that answers a question in `webclaw --map --map-source sitemap,cc`, without `--query` (the arm above) and with it (a second run, so the index may answer differently; Crawl4AI ranks by a query only after fetching the `<head>` of every URL, so it has no row here):

| site | question | target | URLs without | rank without | URLs with | rank with --query |
|---|---|---|---:|---:|---:|---:|
| docs.python.org | how do I cancel an asyncio task | docs.python.org/3/library/asyncio-task.html | 8,562 | 566 | 9,032 | 1 |
| www.sqlite.org | write-ahead log mode | sqlite.org/wal.html | 1,960 | 398 | 1,957 | 402 |
| go.dev | tutorial on generics | go.dev/doc/tutorial/generics | 3,600 | 3521 | 1,029 | 2 |
| kubernetes.io | configure liveness probes | kubernetes.io/docs/tasks/configure-pod-container/configure-liveness-readiness-startup-probes | 6,619 | 783 | 6,619 | 1 |
