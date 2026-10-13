# Response cache 2026-10-13-cache

Each tool fetches the 78 url-mode cases twice through its own disk cache, starting empty: webclaw `--cache -f llm --render never` (one process per page, 8 at a time), Scrapling's Spider in `development_mode`, Crawl4AI `CacheMode.ENABLED` with `arun_many`. The second run sends every request to a proxy that refuses connections, so `served from cache` counts pages answered without the network. `identical` counts second-run pages whose text matches the first run.

| tool | first run ok | first run s | second run s | served from cache | identical |
|---|---:|---:|---:|---:|---:|
| webclaw | 68/78 | 4.6 | 1.8 | 68/68 | 68/68 |
| scrapling | 67/78 | 9.9 | 5.7 | 67/67 | 67/67 |
| crawl4ai | 66/78 | 17.7 | 2.6 | 66/66 | 66/66 |
