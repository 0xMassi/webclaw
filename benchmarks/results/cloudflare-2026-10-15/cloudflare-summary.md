# Cloudflare challenges cloudflare-2026-10-15

Each tool loads 16 pages behind a Cloudflare challenge or with a Turnstile widget, one fresh browser per page, 60 s budget per page. A challenge page counts as solved when the tool's output holds a text marker from the real page and not the challenge's `Just a moment...` title. An embedded Turnstile widget counts as solved when the page holds a widget token. ms is the median over solved pages. `reuse` counts solved challenge pages that the tool's own plain HTTP client (no browser) then loads in the same process, at the page's URL with `?again=1`, with the browser's cookies and User-Agent, under the same marker check: webclaw as the second URL of one `--solve` run (`--concurrency 1`, counted only when it sent the clearance and got no new challenge), Scrapling with its `Fetcher` (`impersonate="chrome"`). webclaw `--solve` loads embedded widgets with `--render always` and reads the token with an `executeJavascript` step.

| tool | non-interactive (2) | managed (10) | embedded (4) | all | ms (median) | reuse |
|---|---:|---:|---:|---:|---:|---:|
| webclaw-http | 0/2 | 0/10 | 0/4 | 0/16 | 0 | - |
| webclaw-render | 0/2 | 0/10 | 0/4 | 0/16 | 0 | - |
| webclaw-solve | 2/2 | 10/10 | 3/4 | 15/16 | 4236 | 5/12 |
| scrapling | 2/2 | 10/10 | 2/4 | 14/16 | 3654 | 9/12 |
| scrapling-real-chrome | 0/2 | 3/10 | 2/4 | 5/16 | 3961 | 1/3 |
| crawl4ai | 0/2 | 0/10 | 2/4 | 2/16 | 6140 | - |

Per page (solved, ms, reuse):

| page | kind | webclaw-http | webclaw-render | webclaw-solve | scrapling | scrapling-real-chrome | crawl4ai |
|---|---|---|---|---|---|---|---|
| https://www.scrapingcourse.com/cloudflare-challenge | managed | no | no | yes 7.1s, reused | yes 16.2s, reused | no | no |
| https://www.scrapingcourse.com/antibot-challenge | managed | no | no | yes 7.2s, reused | yes 3.7s, reused | no | no |
| https://demo.turnstile.workers.dev/ | embedded | no | no | no | no | no | yes 6.0s |
| https://seleniumbase.io/apps/turnstile | embedded | no | no | yes 4.1s | yes 1.7s | yes 1.9s | no |
| https://peet.ws/turnstile-test/non-interactive.html | embedded | no | no | yes 3.0s | yes 2.7s | yes 2.8s | yes 6.3s |
| https://peet.ws/turnstile-test/managed.html | embedded | no | no | yes 4.2s | no | no | no |
| https://www.bscscan.com/ | non-interactive | no | no | yes 4.1s, not reused | yes 3.5s, not reused | no | no |
| https://basescan.org/ | non-interactive | no | no | yes 4.3s, reused | yes 3.4s, reused | no | no |
| https://gitlab.com/users/sign_in | managed | no | no | yes 4.1s, reused | yes 4.7s, reused | yes 6.6s, reused | no |
| https://www.glassdoor.com/Reviews/index.htm | managed | no | no | yes 6.3s, not reused | yes 3.4s, not reused | yes 4.0s, not reused | no |
| https://www.crunchbase.com/organization/openai | managed | no | no | yes 4.1s, not reused | yes 3.5s, reused | no | no |
| https://www.cardmarket.com/en/Magic | managed | no | no | yes 4.4s, not reused | yes 5.0s, reused | no | no |
| https://www.bizjournals.com/ | managed | no | no | yes 4.6s, not reused | yes 6.8s, reused | no | no |
| https://www.pricecharting.com/ | managed | no | no | yes 3.9s, reused | yes 3.7s, reused | no | no |
| https://www.bbb.org/ | managed | no | no | yes 4.4s, not reused | yes 3.7s, reused | no | no |
| https://www.capterra.com/ | managed | no | no | yes 2.7s, not reused | yes 3.7s, not reused | yes 4.4s, not reused | no |
