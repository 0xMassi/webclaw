# Cloudflare challenges cloudflare-2026-10-15

Each tool loads 17 pages behind a Cloudflare challenge or with a Turnstile widget, one fresh browser per page, 60 s budget per page. A challenge page counts as solved when the tool's output holds a text marker from the real page and not the challenge's `Just a moment...` title. An embedded Turnstile widget counts as solved when the page holds a widget token. ms is the median over solved pages. `reuse` counts solved challenge pages whose browser cookies, sent with the browser's User-Agent, let webclaw's plain HTTP client (no browser) load the page straight after: for webclaw `--solve`, the second of two URLs of the page in one run (`--concurrency 1`); for Scrapling, a separate webclaw run handed its cookies and User-Agent, with the `chrome-macos` profile when that User-Agent is macOS. `reuse, own UA` sends Scrapling's cookies with webclaw's own User-Agent instead. webclaw `--solve` loads embedded widgets with `--render always` and reads the token with an `executeJavascript` step.

| tool | non-interactive (2) | managed (10) | interactive (1) | embedded (4) | all | ms (median) | reuse | reuse, own UA |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-http | 0/2 | 0/10 | 0/1 | 0/4 | 0/17 | 0 | - | - |
| webclaw-render | 0/2 | 0/10 | 0/1 | 0/4 | 0/17 | 0 | - | - |
| webclaw-solve | 2/2 | 10/10 | 1/1 | 4/4 | 17/17 | 4089 | 6/13 | - |
| scrapling | 2/2 | 10/10 | 1/1 | 2/4 | 15/17 | 3848 | 10/13 | 1/13 |
| crawl4ai | 0/2 | 0/10 | 0/1 | 1/4 | 1/17 | 6081 | - | - |

Per page (solved, ms):

| page | kind | webclaw-http | webclaw-render | webclaw-solve | scrapling | crawl4ai |
|---|---|---|---|---|---|---|
| https://www.scrapingcourse.com/cloudflare-challenge | managed | no | no | yes 9.9s | yes 4.8s | no |
| https://www.scrapingcourse.com/antibot-challenge | managed | no | no | yes 9.6s | yes 4.4s | no |
| https://seleniumbase.io/apps/turnstile | embedded | no | no | yes 3.7s | no | no |
| https://peet.ws/turnstile-test/non-interactive.html | embedded | no | no | yes 2.6s | yes 2.8s | yes 6.1s |
| https://peet.ws/turnstile-test/managed.html | embedded | no | no | yes 4.1s | no | no |
| https://www.bscscan.com/ | non-interactive | no | no | yes 3.9s | yes 3.3s | no |
| https://basescan.org/ | non-interactive | no | no | yes 3.8s | yes 4.3s | no |
| https://gitlab.com/users/sign_in | managed | no | no | yes 4.1s | yes 4.6s | no |
| https://www.glassdoor.com/Reviews/index.htm | managed | no | no | yes 13.9s | yes 3.3s | no |
| https://www.crunchbase.com/organization/openai | managed | no | no | yes 4.0s | yes 3.3s | no |
| https://www.cardmarket.com/en/Magic | managed | no | no | yes 4.1s | yes 4.2s | no |
| https://www.bizjournals.com/ | managed | no | no | yes 4.4s | yes 62.4s | no |
| https://www.pricecharting.com/ | managed | no | no | yes 3.7s | yes 3.4s | no |
| https://www.bbb.org/ | managed | no | no | yes 4.0s | yes 3.6s | no |
| https://www.capterra.com/ | managed | no | no | yes 2.5s | yes 3.8s | no |
