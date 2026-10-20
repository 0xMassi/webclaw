# Presenting as regular Chrome, chrome-ua-2026-10-15

3 rounds, a fresh browser per page. Tells come from the httpbin page (headers the browser sent, plus what the page and a dedicated worker read); the bot check is deviceandbrowserinfo.com's verdict and the signals it caught; Nasdaq and Bundlephobia count when the page called its data API (the AAPL quote, the size of react), as the browser's resource timing lists it. Median seconds include the probe's waits (5 s on the bot check, 2 s on Nasdaq and Bundlephobia).

| tool | headless tells | bot check | Nasdaq | Bundlephobia | median s |
|---|---|---|---:|---:|---:|
| webclaw | high-entropy client hints blank | human | 3/3 | 3/3 | 4.36 |
| webclaw MCP `scrape` | high-entropy client hints blank | human | 3/3 | 3/3 | 4.38 |
| webclaw before | UA says Headless; worker UA says Headless | bot: hasBotUserAgent | 0/3 | 0/3 | 4.20 |
| webclaw before, `--solve` | high-entropy client hints blank | human | 3/3 | 3/3 | 5.31 |
| Scrapling DynamicFetcher | architecture differs from the machine's; navigator.webdriver is true | bot: hasInconsistentTimingResolution hasWebdriverInFrameTrue hasWebdriverTrue isAutomatedWithCDP isAutomatedWithCDPInWebWorker, bot: hasWebdriverInFrameTrue hasWebdriverTrue isAutomatedWithCDP isAutomatedWithCDPInWebWorker | 3/3 | 3/3 | 3.25 |
| Scrapling StealthyFetcher | architecture differs from the machine's | human | 3/3 | 3/3 | 3.18 |
| Crawl4AI | UA version differs from client hints; architecture differs from the machine's; client hints say Headless; high-entropy client hints blank; platform differs from the machine's | bot: hasInconsistentChromeObject hasInconsistentTimingResolution isAutomatedWithCDP isAutomatedWithCDPInWebWorker | 3/3 | 3/3 | 3.01 |
| Crawl4AI `enable_stealth` | UA version differs from client hints; architecture differs from the machine's; client hints say Headless; high-entropy client hints blank; platform differs from the machine's; worker differs from page | no result | 3/3 | 3/3 | 3.03 |

webclaw's view on this machine: `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36`, brands Google Chrome/155, Chromium/155, Not(A:Brand/24

## Launch cost

One `--render always` render of example.com per process, 5 times.

| build | median s | min s | max s |
|---|---:|---:|---:|
| webclaw | 1.45 | 1.43 | 1.67 |
| webclaw before | 1.41 | 1.37 | 1.45 |
| webclaw before, `--solve` | 2.34 | 2.28 | 2.42 |

## Render slice of the extraction benchmark

`run.py` with the webclaw-auto (`--render auto`) and webclaw-render (`--render always`) arms, 3 runs of every case.

| build | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
| webclaw | webclaw-auto | 234 | 90 | 77.6 | 3255 | 0.27 | 2.47 |
| webclaw | webclaw-render | 234 | 82 | 73.8 | 1904 | 1.95 | 2.59 |
| webclaw before | webclaw-auto | 234 | 90 | 77.6 | 3255 | 0.25 | 2.40 |
| webclaw before | webclaw-render | 234 | 77 | 70.0 | 2226 | 1.88 | 2.62 |
