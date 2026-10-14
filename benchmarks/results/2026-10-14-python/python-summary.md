# Python packages 2026-10-14-python

Each tool is installed into a fresh Python 3.12 venv with `uv pip install` and an empty uv cache, then its documented post-install step runs (`scrapling install`, `crawl4ai-setup`; they download Chromium into a fresh browser directory). webclaw installs the wheel built from this checkout, so it downloads nothing. `other MB` is the browser directory plus Crawl4AI's home directory.

| tool | version | install s | post-install s | packages | venv MB | other MB | total MB |
|---|---|---:|---:|---:|---:|---:|---:|
| webclaw-cli (local wheel) | 0.6.24 | 0.1 | 0.0 | 1 | 29 | 0 | 29 |
| Scrapling `Fetcher` | 0.4.15 | 3.3 | 16.4 | 26 | 325 | 572 | 897 |
| Crawl4AI `AsyncWebCrawler` | 0.9.4 | 5.5 | 38.6 | 95 | 610 | 1129 | 1739 |

Calls on the 60 static golden pages (`cases.jsonl` without `spa` and `pdf`). `first call` starts a new Python process that imports the tool and scrapes one page, median of 5 pages; `setup` is the import plus creating the client (Crawl4AI starts its browser there). The steady loop keeps one process, makes one warm-up call, then calls each page 2 times. webclaw returns `llm` text, with the default `render='auto'` and again with `render='never'`; Scrapling `markdown()`, Crawl4AI `raw_markdown`. Peak RSS sums the Python process and all its children, sampled every 20 ms.

| tool | first call s | setup s | first call peak MB | steady ok | steady p50 s | steady p95 s | fact recall % | steady peak MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw-cli (local wheel) | 0.63 | 0.01 | 44 | 104/120 | 0.28 | 0.65 | 75.9 | 93 |
| webclaw-cli, `render='never'` | 0.29 | 0.00 | 41 | 105/120 | 0.23 | 0.61 | 76.4 | 91 |
| Scrapling `Fetcher` | 0.74 | 0.07 | 65 | 118/120 | 0.32 | 1.04 | 83.9 | 317 |
| Crawl4AI `AsyncWebCrawler` | 1.70 | 0.47 | 645 | 113/120 | 0.45 | 1.12 | 82.8 | 1700 |
