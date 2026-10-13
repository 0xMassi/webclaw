# Browser profiles 2026-10-13-profiles

Each tool fetches the 69 non-PDF url-mode cases once per browser identity, over HTTP only (webclaw `--render never -f llm`, Scrapling `Fetcher.get(impersonate=...)` then `.markdown()`). A run is ok when it returns text holding at least one of the case's facts; recall counts all facts. Scrapling has no separate macOS Chrome or random identity.

| profile | tool | ok | fact recall % | p50 s | p95 s |
|---|---|---:|---:|---:|---:|
| chrome | webclaw | 55/69 | 72.4 | 0.39 | 0.84 |
| chrome | scrapling | 60/69 | 79.7 | 0.33 | 2.56 |
| chrome-macos | webclaw | 54/69 | 71.4 | 0.21 | 0.57 |
| firefox | webclaw | 58/69 | 77.1 | 0.28 | 0.64 |
| firefox | scrapling | 60/69 | 79.7 | 0.36 | 2.44 |
| safari | webclaw | 57/69 | 74.5 | 0.23 | 0.56 |
| safari | scrapling | 60/69 | 79.7 | 0.38 | 2.00 |
| safari-ios | webclaw | 57/69 | 74.5 | 0.27 | 0.81 |
| safari-ios | scrapling | 60/69 | 81.2 | 0.34 | 1.75 |
| edge | webclaw | 54/69 | 71.4 | 0.25 | 0.78 |
| edge | scrapling | 58/69 | 78.1 | 0.32 | 2.89 |
| random | webclaw | 57/69 | 74.5 | 0.24 | 0.54 |
