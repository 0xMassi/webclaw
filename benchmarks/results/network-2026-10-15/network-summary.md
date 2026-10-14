# Network capture network-2026-10-15

Each tool renders 11 pages whose data comes from JSON APIs, with a fresh browser per page, and returns the XHR and fetch responses the page received. The ground truth is one API per page and a field in its JSON, checked by calling each API directly: 11/11 answered with the field this run. `found` counts ground-truth APIs whose URL is in the output, `bodies ok` those whose captured body parses as JSON with the field, `noise` the other responses in the output. webclaw runs `--capture-network` (xhr and fetch). Scrapling runs `DynamicFetcher.fetch(capture_xhr=...)` with `network_idle=True`. Crawl4AI runs `capture_network_requests=True` with `wait_until="networkidle"`; it keeps every response of every resource type and has no filter. The filtered rows pass each page's ground-truth pattern as the tool's URL filter.

| tool | pages ok | found | bodies ok | responses | noise | ms (median) | output KB (median) |
|---|---:|---:|---:|---:|---:|---:|---:|
| webclaw | 10/11 | 10/11 | 10/11 | 104 | 83 | 2856 | 85 |
| scrapling | 11/11 | 11/11 | 11/11 | 156 | 123 | 2552 | 217 |
| crawl4ai | 10/11 | 10/11 | 10/11 | 592 | 570 | 2335 | 3185 |
| webclaw (filtered) | 10/11 | 10/11 | 10/11 | 21 | 0 | 2709 | 6 |
| scrapling (filtered) | 11/11 | 11/11 | 11/11 | 32 | 0 | 2358 | 17 |

Bodies ok per page (responses captured):

| page | webclaw | scrapling | crawl4ai | webclaw (filtered) | scrapling (filtered) |
|---|---|---|---|---|---|
| https://quotes.toscrape.com/scroll | 1/1 (1) | 1/1 (1) | 1/1 (6) | 1/1 (1) | 1/1 (1) |
| https://earthquake.usgs.gov/earthquakes/map/ | 1/1 (25) | 1/1 (44) | 1/1 (19) | 1/1 (1) | 1/1 (1) |
| https://www.swpc.noaa.gov/products/planetary-k-index | 1/1 (4) | 1/1 (4) | 1/1 (89) | 1/1 (1) | 1/1 (1) |
| https://www.yr.no/en/forecast/daily-table/2-2643743/United%20Kingdom/England/London/London | 1/1 (9) | 1/1 (9) | 1/1 (52) | 1/1 (1) | 1/1 (1) |
| https://www.nasdaq.com/market-activity/stocks/aapl | fail | 1/1 (30) | fail | fail | 1/1 (10) |
| https://www.aljazeera.com/ | 1/1 (12) | 1/1 (12) | 1/1 (64) | 1/1 (1) | 1/1 (1) |
| https://github.com/rust-lang/cargo | 1/1 (9) | 1/1 (9) | 1/1 (178) | 1/1 (1) | 1/1 (1) |
| https://gitlab.com/gitlab-org/gitlab | 1/1 (22) | 1/1 (22) | 1/1 (87) | 1/1 (11) | 1/1 (11) |
| https://pokeapi.co/ | 1/1 (7) | 1/1 (7) | 1/1 (23) | 1/1 (1) | 1/1 (1) |
| https://bundlephobia.com/package/react | 1/1 (11) | 1/1 (14) | 1/1 (60) | 1/1 (2) | 1/1 (3) |
| https://petstore.swagger.io/ | 1/1 (4) | 1/1 (4) | 1/1 (14) | 1/1 (1) | 1/1 (1) |

Direct API checks:

- https://quotes.toscrape.com/api/quotes?page=1: ok
- https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/2.5_day.geojson: ok
- https://services.swpc.noaa.gov/products/noaa-planetary-k-index.json: ok
- https://www.yr.no/api/v0/locations/2-2643743/forecast: ok
- https://api.nasdaq.com/api/quote/AAPL/info?assetclass=stocks: ok
- https://www.aljazeera.com/graphql?wp-site=aje&operationName=ArchipelagoBreakingTickerQuery&variables=%7B%7D&extensions=%7B%7D: ok
- https://github.com/rust-lang/cargo/latest-commit: ok
- https://gitlab.com/api/graphql: ok
- https://pokeapi.co/api/v2/pokemon/ditto: ok
- https://bundlephobia.com/api/size?package=react&record=true: ok
- https://petstore.swagger.io/v2/swagger.json: ok
