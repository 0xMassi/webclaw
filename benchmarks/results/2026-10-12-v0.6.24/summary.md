# Benchmark 2026-10-12-v0.6.24

webclaw: webclaw 0.6.24
Tool versions: [versions.txt](versions.txt)

Fact recall counts failed runs as 0 hits. Tokens: cl100k median over successful runs. Seconds: wall time per URL.

## url mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
| ALL | crawl4ai-fit | 234 | 84 | 70.0 | 1799 | 0.45 | 2.09 |
| ALL | crawl4ai-raw | 234 | 85 | 78.1 | 8448 | 0.42 | 1.06 |
| ALL | scrapling | 234 | 91 | 74.3 | 4825 | 0.29 | 0.72 |
| ALL | trafilatura | 234 | 74 | 51.4 | 1066 | 0.13 | 0.46 |
| ALL | webclaw-llm | 234 | 91 | 75.9 | 2761 | 0.25 | 0.62 |
| ALL | webclaw-md | 234 | 91 | 74.3 | 4267 | 0.23 | 0.58 |
| docs | crawl4ai-fit | 27 | 100 | 100.0 | 7680 | 0.47 | 0.92 |
| docs | crawl4ai-raw | 27 | 100 | 100.0 | 11367 | 0.34 | 0.82 |
| docs | scrapling | 27 | 100 | 100.0 | 9895 | 0.13 | 0.77 |
| docs | trafilatura | 27 | 100 | 100.0 | 7865 | 0.06 | 0.26 |
| docs | webclaw-llm | 27 | 100 | 100.0 | 7282 | 0.10 | 0.64 |
| docs | webclaw-md | 27 | 100 | 100.0 | 8256 | 0.10 | 0.30 |
| ecommerce | crawl4ai-fit | 27 | 100 | 100.0 | 651 | 0.31 | 0.79 |
| ecommerce | crawl4ai-raw | 27 | 100 | 100.0 | 1095 | 0.30 | 0.89 |
| ecommerce | scrapling | 27 | 100 | 100.0 | 956 | 0.43 | 0.54 |
| ecommerce | trafilatura | 27 | 100 | 100.0 | 581 | 0.13 | 0.17 |
| ecommerce | webclaw-llm | 27 | 100 | 100.0 | 1012 | 0.41 | 0.55 |
| ecommerce | webclaw-md | 27 | 100 | 100.0 | 932 | 0.41 | 0.55 |
| homepage | crawl4ai-fit | 54 | 96 | 64.4 | 1348 | 0.60 | 1.42 |
| homepage | crawl4ai-raw | 54 | 100 | 75.6 | 5395 | 0.56 | 1.10 |
| homepage | scrapling | 54 | 100 | 77.8 | 3558 | 0.22 | 0.56 |
| homepage | trafilatura | 54 | 70 | 35.6 | 448 | 0.16 | 0.42 |
| homepage | webclaw-llm | 54 | 89 | 67.0 | 1374 | 0.23 | 0.58 |
| homepage | webclaw-md | 54 | 89 | 66.7 | 2698 | 0.22 | 0.57 |
| news | crawl4ai-fit | 27 | 100 | 100.0 | 5158 | 0.42 | 0.62 |
| news | crawl4ai-raw | 27 | 100 | 100.0 | 10381 | 0.36 | 0.60 |
| news | scrapling | 27 | 100 | 100.0 | 10039 | 0.14 | 0.20 |
| news | trafilatura | 27 | 100 | 100.0 | 1756 | 0.05 | 0.17 |
| news | webclaw-llm | 27 | 100 | 100.0 | 3532 | 0.12 | 0.14 |
| news | webclaw-md | 27 | 100 | 100.0 | 3429 | 0.12 | 0.20 |
| nonenglish | crawl4ai-fit | 27 | 100 | 100.0 | 145099 | 1.98 | 3.74 |
| nonenglish | crawl4ai-raw | 27 | 100 | 100.0 | 217962 | 0.95 | 1.52 |
| nonenglish | scrapling | 27 | 100 | 100.0 | 202776 | 0.57 | 1.14 |
| nonenglish | trafilatura | 27 | 100 | 100.0 | 108598 | 0.35 | 0.81 |
| nonenglish | webclaw-llm | 27 | 100 | 100.0 | 74029 | 0.35 | 0.53 |
| nonenglish | webclaw-md | 27 | 100 | 100.0 | 167382 | 0.32 | 0.80 |
| paywalled | crawl4ai-fit | 18 | 50 | 50.0 | 1105 | 0.57 | 0.94 |
| paywalled | crawl4ai-raw | 18 | 50 | 50.0 | 8448 | 0.45 | 0.89 |
| paywalled | scrapling | 18 | 83 | 33.3 | 13 | 0.19 | 3.39 |
| paywalled | trafilatura | 18 | 0 | 0.0 | 0 | 0.10 | 0.13 |
| paywalled | webclaw-llm | 18 | 50 | 50.0 | 1803 | 0.15 | 0.82 |
| paywalled | webclaw-md | 18 | 50 | 50.0 | 2167 | 0.15 | 0.77 |
| pdf | crawl4ai-fit | 27 | 0 | 0.0 | 0 | 0.06 | 0.10 |
| pdf | crawl4ai-raw | 27 | 0 | 0.0 | 0 | 0.06 | 0.10 |
| pdf | scrapling | 27 | 33 | 16.7 | 304113 | 0.32 | 0.48 |
| pdf | trafilatura | 27 | 0 | 0.0 | 0 | 0.08 | 0.28 |
| pdf | webclaw-llm | 27 | 100 | 94.4 | 17043 | 0.31 | 0.67 |
| pdf | webclaw-md | 27 | 100 | 94.4 | 17691 | 0.24 | 0.46 |
| spa | crawl4ai-fit | 27 | 100 | 61.1 | 311 | 0.44 | 0.80 |
| spa | crawl4ai-raw | 27 | 100 | 100.0 | 412 | 0.43 | 0.71 |
| spa | scrapling | 27 | 100 | 38.9 | 75 | 0.26 | 0.42 |
| spa | trafilatura | 27 | 100 | 22.2 | 14 | 0.14 | 0.47 |
| spa | webclaw-llm | 27 | 78 | 22.2 | 35 | 0.23 | 0.44 |
| spa | webclaw-md | 27 | 78 | 5.6 | 6 | 0.21 | 0.42 |

## html mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
| ALL | crawl4ai-fit | 69 | 97 | 73.4 | 1223 | 0.06 | 2.17 |
| ALL | crawl4ai-raw | 69 | 99 | 78.6 | 6756 | 0.04 | 1.00 |
| ALL | scrapling | 69 | 100 | 78.1 | 4363 | 0.02 | 0.39 |
| ALL | trafilatura | 69 | 100 | 64.6 | 625 | 0.01 | 0.29 |
| ALL | webclaw-llm | 69 | 100 | 75.0 | 1478 | 0.01 | 0.15 |
| ALL | webclaw-md | 69 | 97 | 73.4 | 2184 | 0.01 | 0.11 |
| docs | crawl4ai-fit | 9 | 100 | 100.0 | 7767 | 0.13 | 0.25 |
| docs | crawl4ai-raw | 9 | 100 | 100.0 | 12175 | 0.04 | 0.12 |
| docs | scrapling | 9 | 100 | 100.0 | 9895 | 0.04 | 0.05 |
| docs | trafilatura | 9 | 100 | 100.0 | 7865 | 0.02 | 0.04 |
| docs | webclaw-llm | 9 | 100 | 100.0 | 6429 | 0.02 | 0.03 |
| docs | webclaw-md | 9 | 100 | 100.0 | 7801 | 0.01 | 0.02 |
| ecommerce | crawl4ai-fit | 9 | 100 | 100.0 | 627 | 0.02 | 0.02 |
| ecommerce | crawl4ai-raw | 9 | 100 | 100.0 | 949 | 0.01 | 0.01 |
| ecommerce | scrapling | 9 | 100 | 100.0 | 956 | 0.00 | 0.01 |
| ecommerce | trafilatura | 9 | 100 | 100.0 | 581 | 0.00 | 0.00 |
| ecommerce | webclaw-llm | 9 | 100 | 100.0 | 967 | 0.01 | 0.01 |
| ecommerce | webclaw-md | 9 | 100 | 100.0 | 814 | 0.01 | 0.01 |
| homepage | crawl4ai-fit | 18 | 100 | 65.6 | 898 | 0.07 | 0.92 |
| homepage | crawl4ai-raw | 18 | 100 | 75.6 | 6099 | 0.06 | 0.30 |
| homepage | scrapling | 18 | 100 | 72.2 | 3514 | 0.02 | 0.04 |
| homepage | trafilatura | 18 | 100 | 46.7 | 355 | 0.01 | 0.05 |
| homepage | webclaw-llm | 18 | 100 | 68.9 | 1153 | 0.02 | 0.03 |
| homepage | webclaw-md | 18 | 100 | 68.9 | 2473 | 0.02 | 0.03 |
| news | crawl4ai-fit | 9 | 100 | 100.0 | 5004 | 0.10 | 0.88 |
| news | crawl4ai-raw | 9 | 100 | 100.0 | 10227 | 0.04 | 0.52 |
| news | scrapling | 9 | 100 | 100.0 | 10039 | 0.03 | 0.07 |
| news | trafilatura | 9 | 100 | 100.0 | 1756 | 0.01 | 0.07 |
| news | webclaw-llm | 9 | 100 | 100.0 | 3502 | 0.01 | 0.02 |
| news | webclaw-md | 9 | 100 | 100.0 | 3429 | 0.01 | 0.02 |
| nonenglish | crawl4ai-fit | 9 | 100 | 100.0 | 142484 | 1.74 | 27.93 |
| nonenglish | crawl4ai-raw | 9 | 100 | 100.0 | 208489 | 0.87 | 40.19 |
| nonenglish | scrapling | 9 | 100 | 100.0 | 202776 | 0.37 | 0.72 |
| nonenglish | trafilatura | 9 | 100 | 100.0 | 108598 | 0.27 | 0.59 |
| nonenglish | webclaw-llm | 9 | 100 | 100.0 | 64494 | 0.14 | 0.29 |
| nonenglish | webclaw-md | 9 | 100 | 100.0 | 148511 | 0.11 | 0.23 |
| paywalled | crawl4ai-fit | 6 | 100 | 50.0 | 504 | 0.01 | 0.06 |
| paywalled | crawl4ai-raw | 6 | 100 | 50.0 | 3364 | 0.01 | 0.27 |
| paywalled | scrapling | 6 | 100 | 50.0 | 3059 | 0.00 | 0.02 |
| paywalled | trafilatura | 6 | 100 | 50.0 | 190 | 0.00 | 0.01 |
| paywalled | webclaw-llm | 6 | 100 | 50.0 | 890 | 0.01 | 0.01 |
| paywalled | webclaw-md | 6 | 100 | 50.0 | 1081 | 0.01 | 0.01 |
| spa | crawl4ai-fit | 9 | 78 | 22.2 | 9 | 0.01 | 0.01 |
| spa | crawl4ai-raw | 9 | 89 | 27.8 | 70 | 0.01 | 0.01 |
| spa | scrapling | 9 | 100 | 38.9 | 75 | 0.00 | 0.00 |
| spa | trafilatura | 9 | 100 | 22.2 | 14 | 0.00 | 0.06 |
| spa | webclaw-llm | 9 | 100 | 22.2 | 25 | 0.01 | 0.01 |
| spa | webclaw-md | 9 | 78 | 5.6 | 6 | 0.00 | 0.01 |
