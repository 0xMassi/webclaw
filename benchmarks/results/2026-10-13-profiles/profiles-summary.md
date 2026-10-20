# Browser profiles 2026-10-13-profiles

Each tool fetches the 69 non-PDF url-mode cases once per browser identity, over HTTP only (webclaw `--render never -f llm`, Scrapling `Fetcher.get(impersonate=...)` then `.markdown()`). A run is ok when it returns text holding at least one of the case's facts; recall counts all facts. Scrapling has no separate macOS Chrome or random identity.

| profile | tool | ok | fact recall % | p50 s | p95 s |
|---|---|---:|---:|---:|---:|
| chrome | webclaw | 55/69 | 71.9 | 0.33 | 1.04 |
| chrome | scrapling | 60/69 | 78.6 | 0.36 | 1.65 |
| chrome-macos | webclaw | 55/69 | 71.9 | 0.25 | 0.53 |
| firefox | webclaw | 55/69 | 73.4 | 0.22 | 0.57 |
| firefox | scrapling | 59/69 | 77.6 | 0.32 | 2.16 |
| safari | webclaw | 57/69 | 74.5 | 0.24 | 0.59 |
| safari | scrapling | 60/69 | 78.6 | 0.31 | 1.88 |
| safari-ios | webclaw | 57/69 | 74.0 | 0.26 | 0.71 |
| safari-ios | scrapling | 58/69 | 78.1 | 0.31 | 1.80 |
| edge | webclaw | 56/69 | 72.9 | 0.24 | 0.70 |
| edge | scrapling | 58/69 | 77.1 | 0.31 | 2.30 |
| random | webclaw | 55/69 | 72.4 | 0.22 | 0.55 |
