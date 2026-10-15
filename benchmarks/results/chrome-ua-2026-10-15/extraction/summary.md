# Benchmark chrome-ua-2026-10-15/extraction

webclaw: webclaw 0.6.24 +browser
Tool versions: [versions.txt](versions.txt)

Fact recall counts failed runs as 0 hits. Tokens: cl100k median over successful runs. Seconds: wall time per URL.

## url mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
| ALL | webclaw-auto | 234 | 90 | 77.6 | 3255 | 0.27 | 2.47 |
| ALL | webclaw-render | 234 | 82 | 73.8 | 1904 | 1.95 | 2.59 |
| docs | webclaw-auto | 27 | 100 | 100.0 | 7282 | 0.11 | 0.66 |
| docs | webclaw-render | 27 | 100 | 100.0 | 6738 | 1.57 | 2.60 |
| ecommerce | webclaw-auto | 27 | 100 | 100.0 | 1012 | 0.41 | 0.52 |
| ecommerce | webclaw-render | 27 | 100 | 100.0 | 1012 | 2.27 | 2.41 |
| homepage | webclaw-auto | 54 | 89 | 66.7 | 1373 | 0.22 | 0.66 |
| homepage | webclaw-render | 54 | 89 | 70.0 | 1477 | 2.03 | 3.66 |
| news | webclaw-auto | 27 | 100 | 100.0 | 3532 | 0.11 | 0.59 |
| news | webclaw-render | 27 | 100 | 100.0 | 3532 | 1.87 | 2.18 |
| nonenglish | webclaw-auto | 27 | 100 | 100.0 | 74007 | 0.34 | 0.68 |
| nonenglish | webclaw-render | 27 | 100 | 100.0 | 73964 | 2.06 | 2.64 |
| paywalled | webclaw-auto | 18 | 0 | 0.0 | 0 | 0.11 | 0.14 |
| paywalled | webclaw-render | 18 | 50 | 50.0 | 1803 | 1.87 | 2.37 |
| pdf | webclaw-auto | 27 | 100 | 94.4 | 17043 | 0.26 | 0.45 |
| pdf | webclaw-render | 27 | 0 | 0.0 | 0 | 1.68 | 1.73 |
| spa | webclaw-auto | 27 | 100 | 77.8 | 323 | 2.32 | 2.63 |
| spa | webclaw-render | 27 | 100 | 77.8 | 323 | 2.16 | 2.40 |

## html mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
