# Python packages 2026-10-14-python-v2

Each tool's wheels are first saved in a local folder (`download MB`). Then the tool is installed from that folder with no network and an empty uv cache, into a fresh Python 3.12 venv, 3 times (median). The documented post-install step runs once after that, over the network: `scrapling install` and `crawl4ai-setup` download Chromium into a fresh browser directory. Scrapling's plain `Fetcher`, used below, works without that download, so it also has a row without it. `other MB` is the browser directory plus Crawl4AI's home directory. webclaw renders with a Chrome or Chromium already installed on the machine, which is not counted here.

| tool | version | download MB | install s | post-install s | packages | venv MB | other MB | total MB |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| webclaw-cli (local wheel) | 0.6.24 | 13 | 0.12 | 0.0 | 1 | 29 | 0 | 29 |
| Scrapling, no browser download | 0.4.15 | 96 | 0.29 | 0.0 | 26 | 300 | 0 | 300 |
| Scrapling, with `scrapling install` | 0.4.15 | 96 | 0.29 | 17.7 | 26 | 300 | 578 | 878 |
| Crawl4AI, with `crawl4ai-setup` | 0.9.4 | 170 | 0.83 | 37.9 | 95 | 566 | 1138 | 1705 |

Calls on the 60 static golden pages (`cases.jsonl` without `spa` and `pdf`). `first call` starts a new Python process that imports the tool and scrapes one page, median of 5 pages; `setup` is the import plus creating the client (Crawl4AI starts its browser there). The steady loop keeps one process, makes one warm-up call, then calls each page 2 times. webclaw returns `llm` text, Scrapling `markdown()`, Crawl4AI `raw_markdown`. Peak RSS sums the Python process and all its children, sampled every 20 ms.

| tool | first call s | setup s | first call peak MB | steady ok | steady p50 s | steady p95 s | fact recall % | steady peak MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-cli (local wheel) | 0.59 | 0.01 | 46 | 109/120 | 0.27 | 0.74 | 78.7 | 92 |
| Scrapling `Fetcher` | 0.74 | 0.07 | 67 | 118/120 | 0.27 | 0.85 | 83.9 | 311 |
| Crawl4AI `AsyncWebCrawler` | 1.72 | 0.47 | 650 | 113/120 | 0.46 | 1.06 | 82.8 | 1625 |

Fact recall on the 106 steady calls every tool answered: webclaw-cli (local wheel) 87.5%, Scrapling `Fetcher` 89.2%, Crawl4AI `AsyncWebCrawler` 87.9%.

Failed steady calls (both rounds), with the error each tool reported:

- webclaw-cli (local wheel): https://simonwillison.net/2026/Mar/15/latent-reasoning/ (2x): WebclawError: the target returned HTTP 404 instead of a successful page; https://www.economist.com/business/2026/09/10/are-you-committed-to-your-company-or-your-team (2x): WebclawError: the target returned HTTP 403 instead of a successful page; https://www.economist.com/finance-and-economics/2026/09/12/wanted-new-business-finance-and-economics-interns (2x): WebclawError: the target returned HTTP 403 instead of a successful page; https://www.economist.com/science-and-technology/2026/09/11/top-mathematicians-are-outraged-by-openais-methods (2x): WebclawError: the target returned HTTP 403 instead of a successful page; https://www.ft.com/content/b7fe0fe0-0463-4f55-9590-0a7d08d8fe66 (1x): WebclawError: the target returned HTTP 403 instead of a successful page; https://www.hashicorp.com (2x): WebclawError: the target returned HTTP 429 instead of a successful page
- Scrapling `Fetcher`: https://www.ft.com/content/3bafb3ae-ba94-44f0-984c-ec7c9f54a5e0 (2x): CookieConflict: Multiple cookies exist with name=__cf_bm on router.ft.com and access.ft.com, add domain parameter to sup
- Crawl4AI `AsyncWebCrawler`: https://openai.com (1x): RuntimeError: Blocked by anti-bot protection: Cloudflare JS challenge; https://www.economist.com/business/2026/09/10/are-you-committed-to-your-company-or-your-team (2x): RuntimeError: Blocked by anti-bot protection: DataDome captcha; https://www.economist.com/finance-and-economics/2026/09/12/wanted-new-business-finance-and-economics-interns (2x): RuntimeError: Blocked by anti-bot protection: DataDome captcha; https://www.economist.com/science-and-technology/2026/09/11/top-mathematicians-are-outraged-by-openais-methods (2x): RuntimeError: Blocked by anti-bot protection: DataDome captcha
