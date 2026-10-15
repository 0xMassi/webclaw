# Citations benchmark citations-2026-10-15-v2

webclaw: webclaw 0.6.24 +browser; other tool versions: [versions.txt](versions.txt)

69 pages frozen by `run.sh`. html mode reads them from disk without a page URL, 3 rounds; url mode fetches each live page once per output. Tokens: cl100k. Seconds: median per page. Scrapling has no citation output (its markdown comes from markdownify with links inline), so it has no row.

Saved % is over the total of all pages; the median page saving and the top 5 pages' share of the total saving show how even it is.

| mode | arm | pages | median tokens plain | median tokens cited | total tokens plain | total tokens cited | saved % | median page saved % | pages longer | top 5 pages' share of saving % | median body tokens | fact recall plain % | fact recall cited % |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| html | webclaw | 69 | 2,167 | 2,240 | 1,607,647 | 1,521,283 | 5.4 | 0.0 | 32 | 76.4 | 1,143 | 73.4 | 73.4 |
| html | crawl4ai | 69 | 6,453 | 7,054 | 2,497,781 | 2,803,197 | -12.2 | -23.2 | 68 | - | 3,889 | 77.6 | 77.6 |
| url | webclaw | 63 | 2,676 | 2,816 | 1,723,319 | 1,538,409 | 10.7 | 2.5 | 18 | 68.2 | 1,314 | 85.6 | 85.6 |
| url | crawl4ai | 67 | 8,424 | 9,081 | 2,730,690 | 3,013,380 | -10.4 | -16.5 | 63 | - | 4,066 | 88.3 | 88.3 |

Mark style: the same webclaw citation output, its footnote marks `[^1]` swapped for the other tool's `⟨1⟩` (body and list, same heading), counted again:

| mode | pages | total tokens plain | footnote marks | saved % | angle marks | saved % |
|---|---:|---:|---:|---:|---:|---:|
| html | 69 | 1,607,647 | 1,521,283 | 5.4 | 1,593,000 | 0.9 |
| url | 63 | 1,723,319 | 1,538,409 | 10.7 | 1,610,233 | 6.6 |

html mode:

| arm | links | URL in references % | text then right number % | same-page anchors dropped | images | images cited % | numbered 1..N, one per URL, by first appearance | citation marks inside code blocks | p50 s plain | p50 s cited | p50 ms citation step |
|---|---:|---:|---:|---:|---:|---:|---|---:|---:|---:|---:|
| webclaw | 31,576 | 100.0 | 100.0 | 6,694 | 780 | 100.0 | 69/69 pages | 0 in 378 blocks | 0.013 | 0.013 | - |
| crawl4ai | 52,610 | 100.0 | 100.0 | 0 | 834 | 99.6 | 67/69 pages | 0 in 352 blocks | 0.039 | 0.039 | 0.52 |

Failed pages:

- url webclaw article02: RuntimeError: the target returned HTTP 403 instead of a successful page
- url webclaw article04: RuntimeError: the target returned HTTP 403 instead of a successful page
- url webclaw article05: RuntimeError: the target returned HTTP 403 instead of a successful page
- url webclaw article06: RuntimeError: the target returned HTTP 403 instead of a successful page
- url webclaw home13: RuntimeError: the target returned HTTP 404 instead of a successful page
- url webclaw home17: RuntimeError: the target returned HTTP 429 instead of a successful page
- url crawl4ai article04: RuntimeError: Blocked by anti-bot protection: DataDome captcha
- url crawl4ai article05: RuntimeError: Blocked by anti-bot protection: DataDome captcha
