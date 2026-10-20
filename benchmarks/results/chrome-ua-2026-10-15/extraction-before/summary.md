# Benchmark chrome-ua-2026-10-15/extraction-before

webclaw: webclaw 0.6.24 +browser
Tool versions: [versions.txt](versions.txt)

Fact recall counts failed runs as 0 hits. Tokens: cl100k median over successful runs. Seconds: wall time per URL.

## url mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
| ALL | webclaw-auto | 234 | 90 | 77.6 | 3255 | 0.25 | 2.40 |
| ALL | webclaw-render | 234 | 77 | 70.0 | 2226 | 1.88 | 2.62 |
| docs | webclaw-auto | 27 | 100 | 100.0 | 7282 | 0.10 | 0.33 |
| docs | webclaw-render | 27 | 100 | 100.0 | 6738 | 1.59 | 2.24 |
| ecommerce | webclaw-auto | 27 | 100 | 100.0 | 1012 | 0.40 | 0.54 |
| ecommerce | webclaw-render | 27 | 100 | 100.0 | 1012 | 2.26 | 2.41 |
| homepage | webclaw-auto | 54 | 89 | 66.7 | 1373 | 0.20 | 0.66 |
| homepage | webclaw-render | 54 | 83 | 67.8 | 1623 | 1.90 | 2.81 |
| news | webclaw-auto | 27 | 100 | 100.0 | 3532 | 0.11 | 0.15 |
| news | webclaw-render | 27 | 100 | 100.0 | 3532 | 1.85 | 2.15 |
| nonenglish | webclaw-auto | 27 | 100 | 100.0 | 74007 | 0.34 | 1.06 |
| nonenglish | webclaw-render | 27 | 100 | 100.0 | 73964 | 2.08 | 3.21 |
| paywalled | webclaw-auto | 18 | 0 | 0.0 | 0 | 0.11 | 0.15 |
| paywalled | webclaw-render | 18 | 0 | 0.0 | 0 | 1.63 | 1.88 |
| pdf | webclaw-auto | 27 | 100 | 94.4 | 17043 | 0.27 | 0.47 |
| pdf | webclaw-render | 27 | 0 | 0.0 | 0 | 1.68 | 1.80 |
| spa | webclaw-auto | 27 | 100 | 77.8 | 323 | 2.31 | 2.61 |
| spa | webclaw-render | 27 | 100 | 77.8 | 323 | 2.19 | 2.36 |

## html mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
