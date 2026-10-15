# Citations benchmark citations-2026-10-15

webclaw: webclaw 0.6.24 +browser; other tool versions: [versions.txt](versions.txt)

69 pages frozen by `run.sh`. html mode reads them from disk without a page URL, 3 rounds; url mode fetches each live page once per output. Tokens: cl100k. Seconds: median per page. Scrapling has no citation output (its markdown comes from markdownify with links inline), so it has no row.

| mode | arm | pages | median tokens plain | median tokens cited | total tokens plain | total tokens cited | saved % | median body tokens | fact recall plain % | fact recall cited % |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| html | webclaw | 69 | 2,167 | 2,240 | 1,607,647 | 1,521,246 | 5.4 | 1,143 | 73.4 | 73.4 |
| html | crawl4ai | 69 | 6,453 | 7,054 | 2,497,781 | 2,803,197 | -12.2 | 3,889 | 77.6 | 77.6 |
| url | webclaw | 62 | 2,698 | 2,868 | 1,721,165 | 1,536,140 | 10.7 | 1,325 | 85.5 | 85.5 |
| url | crawl4ai | 67 | 7,861 | 8,820 | 2,721,261 | 3,003,537 | -10.4 | 4,069 | 88.3 | 88.3 |

html mode:

| arm | links | URL in references % | text then right number % | same-page anchors dropped | images | images cited % | numbered 1..N, one per URL, by first appearance | citation marks inside code blocks | p50 s plain | p50 s cited | p50 ms citation step |
|---|---:|---:|---:|---:|---:|---:|---|---:|---:|---:|---:|
| webclaw | 31,576 | 100.0 | 100.0 | 6,694 | 780 | 100.0 | 69/69 pages | 0 in 378 blocks | 0.012 | 0.012 | - |
| crawl4ai | 52,610 | 100.0 | 100.0 | 0 | 834 | 99.6 | 67/69 pages | 0 in 352 blocks | 0.036 | 0.036 | 0.44 |

Failed pages:

- url webclaw article02: RuntimeError: the target returned HTTP 403 instead of a successful page
- url webclaw article03: RuntimeError: the target returned HTTP 403 instead of a successful page
- url webclaw article04: RuntimeError: the target returned HTTP 403 instead of a successful page
- url webclaw article05: RuntimeError: the target returned HTTP 403 instead of a successful page
- url webclaw article06: RuntimeError: the target returned HTTP 403 instead of a successful page
- url webclaw home13: RuntimeError: the target returned HTTP 404 instead of a successful page
- url webclaw home17: RuntimeError: 10-15T06:04:05.425169Z[0m [33m WARN[0m [2mwebclaw_fetch::client[0m[2m:[0m retryable status, will retry [3murl[0m[2m=[0m"https://www.hashicorp.com" [3mstatus[0m[2m=[0m429 [3mattempt_no[0m[2m=[0m1 [3mwait_ms[0m[2m=[0m1000
the target returned HTTP 429 instead of a s
- url crawl4ai article04: RuntimeError: Blocked by anti-bot protection: DataDome captcha
- url crawl4ai article05: RuntimeError: Blocked by anti-bot protection: DataDome captcha
