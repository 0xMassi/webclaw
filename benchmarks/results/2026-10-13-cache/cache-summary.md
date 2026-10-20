# Response cache 2026-10-13-cache

Each tool fetches the 78 url-mode cases twice through its own disk cache, starting empty: webclaw `--cache -f llm --render never` (one process per page, 8 at a time), Scrapling's Spider in `development_mode`, Crawl4AI `CacheMode.ENABLED` with `arun_many`. The second run sends every request to a proxy that refuses connections, so `served from cache` counts pages answered without the network. `identical` counts second-run pages whose text matches the first run.

| tool | first run ok | first run s | second run s | served from cache | identical |
|---|---:|---:|---:|---:|---:|
| webclaw | 68/78 | 4.9 | 1.7 | 68/68 | 68/68 |
| scrapling | 67/78 | 9.0 | 5.7 | 67/67 | 67/67 |
| crawl4ai | 66/78 | 16.0 | 2.7 | 66/66 | 66/66 |
