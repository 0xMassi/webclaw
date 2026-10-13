# Benchmark 2026-10-13-render-everywhere

webclaw: webclaw 0.6.24 +browser
Tool versions: [versions.txt](versions.txt)

Fact recall counts failed runs as 0 hits. Tokens: cl100k median over successful runs. Seconds: wall time per URL.

## url mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
| ALL | crawl4ai-fit | 234 | 84 | 70.0 | 1878 | 0.47 | 2.31 |
| ALL | crawl4ai-raw | 234 | 84 | 76.8 | 8498 | 0.43 | 1.15 |
| ALL | scrapling | 234 | 91 | 74.3 | 4825 | 0.27 | 0.81 |
| ALL | trafilatura | 234 | 75 | 52.2 | 1009 | 0.12 | 0.51 |
| ALL | webclaw-auto | 234 | 94 | 80.5 | 2761 | 0.29 | 1.72 |
| ALL | webclaw-llm | 234 | 91 | 75.7 | 2761 | 0.25 | 0.63 |
| ALL | webclaw-md | 234 | 91 | 74.3 | 4267 | 0.23 | 0.60 |
| ALL | webclaw-render | 234 | 78 | 71.9 | 2068 | 1.36 | 2.01 |
| docs | crawl4ai-fit | 27 | 100 | 100.0 | 7680 | 0.49 | 1.11 |
| docs | crawl4ai-raw | 27 | 100 | 100.0 | 11367 | 0.36 | 0.87 |
| docs | scrapling | 27 | 100 | 100.0 | 9895 | 0.12 | 0.34 |
| docs | trafilatura | 27 | 100 | 100.0 | 7865 | 0.09 | 0.32 |
| docs | webclaw-auto | 27 | 100 | 100.0 | 7282 | 0.10 | 0.62 |
| docs | webclaw-llm | 27 | 100 | 100.0 | 7282 | 0.12 | 0.41 |
| docs | webclaw-md | 27 | 100 | 100.0 | 8256 | 0.10 | 0.38 |
| docs | webclaw-render | 27 | 100 | 100.0 | 6738 | 1.11 | 1.74 |
| ecommerce | crawl4ai-fit | 27 | 100 | 100.0 | 651 | 0.32 | 0.90 |
| ecommerce | crawl4ai-raw | 27 | 100 | 100.0 | 1095 | 0.32 | 0.80 |
| ecommerce | scrapling | 27 | 100 | 100.0 | 956 | 0.41 | 0.52 |
| ecommerce | trafilatura | 27 | 100 | 100.0 | 581 | 0.13 | 0.17 |
| ecommerce | webclaw-auto | 27 | 100 | 100.0 | 1012 | 0.41 | 0.54 |
| ecommerce | webclaw-llm | 27 | 100 | 100.0 | 1012 | 0.42 | 0.54 |
| ecommerce | webclaw-md | 27 | 100 | 100.0 | 932 | 0.44 | 0.54 |
| ecommerce | webclaw-render | 27 | 100 | 100.0 | 1012 | 1.66 | 1.85 |
| homepage | crawl4ai-fit | 54 | 96 | 64.4 | 1348 | 0.64 | 1.57 |
| homepage | crawl4ai-raw | 54 | 96 | 72.6 | 5454 | 0.55 | 1.06 |
| homepage | scrapling | 54 | 100 | 77.8 | 3558 | 0.20 | 0.64 |
| homepage | trafilatura | 54 | 74 | 37.4 | 429 | 0.17 | 0.49 |
| homepage | webclaw-auto | 54 | 89 | 66.7 | 1374 | 0.22 | 0.63 |
| homepage | webclaw-llm | 54 | 89 | 66.7 | 1374 | 0.21 | 0.63 |
| homepage | webclaw-md | 54 | 89 | 66.7 | 2698 | 0.20 | 0.65 |
| homepage | webclaw-render | 54 | 89 | 72.2 | 1532 | 1.35 | 2.17 |
| news | crawl4ai-fit | 27 | 100 | 100.0 | 5134 | 0.46 | 1.14 |
| news | crawl4ai-raw | 27 | 100 | 100.0 | 10362 | 0.39 | 0.58 |
| news | scrapling | 27 | 100 | 100.0 | 10018 | 0.13 | 0.20 |
| news | trafilatura | 27 | 100 | 100.0 | 1756 | 0.05 | 0.56 |
| news | webclaw-auto | 27 | 100 | 100.0 | 3532 | 0.12 | 0.15 |
| news | webclaw-llm | 27 | 100 | 100.0 | 3532 | 0.12 | 0.20 |
| news | webclaw-md | 27 | 100 | 100.0 | 3429 | 0.11 | 0.57 |
| news | webclaw-render | 27 | 100 | 100.0 | 3532 | 1.33 | 1.72 |
| nonenglish | crawl4ai-fit | 27 | 100 | 100.0 | 145099 | 2.28 | 4.09 |
| nonenglish | crawl4ai-raw | 27 | 100 | 100.0 | 217962 | 1.05 | 1.67 |
| nonenglish | scrapling | 27 | 100 | 100.0 | 202776 | 0.65 | 1.29 |
| nonenglish | trafilatura | 27 | 100 | 100.0 | 108598 | 0.39 | 0.93 |
| nonenglish | webclaw-auto | 27 | 100 | 100.0 | 74007 | 0.36 | 1.32 |
| nonenglish | webclaw-llm | 27 | 100 | 100.0 | 74007 | 0.35 | 1.17 |
| nonenglish | webclaw-md | 27 | 100 | 100.0 | 167368 | 0.30 | 1.07 |
| nonenglish | webclaw-render | 27 | 100 | 100.0 | 73964 | 1.46 | 2.22 |
| paywalled | crawl4ai-fit | 18 | 50 | 50.0 | 1105 | 0.74 | 0.96 |
| paywalled | crawl4ai-raw | 18 | 50 | 50.0 | 8473 | 0.42 | 0.84 |
| paywalled | scrapling | 18 | 83 | 33.3 | 13 | 0.12 | 3.42 |
| paywalled | trafilatura | 18 | 0 | 0.0 | 0 | 0.09 | 0.13 |
| paywalled | webclaw-auto | 18 | 50 | 50.0 | 1803 | 0.13 | 0.67 |
| paywalled | webclaw-llm | 18 | 50 | 50.0 | 1803 | 0.13 | 0.72 |
| paywalled | webclaw-md | 18 | 50 | 50.0 | 2167 | 0.13 | 0.65 |
| paywalled | webclaw-render | 18 | 0 | 0.0 | 0 | 1.24 | 1.58 |
| pdf | crawl4ai-fit | 27 | 0 | 0.0 | 0 | 0.07 | 0.10 |
| pdf | crawl4ai-raw | 27 | 0 | 0.0 | 0 | 0.07 | 0.11 |
| pdf | scrapling | 27 | 33 | 16.7 | 304113 | 0.28 | 0.45 |
| pdf | trafilatura | 27 | 0 | 0.0 | 0 | 0.08 | 0.32 |
| pdf | webclaw-auto | 27 | 100 | 94.4 | 17043 | 0.29 | 0.55 |
| pdf | webclaw-llm | 27 | 100 | 94.4 | 17043 | 0.30 | 0.61 |
| pdf | webclaw-md | 27 | 100 | 94.4 | 17691 | 0.30 | 0.57 |
| pdf | webclaw-render | 27 | 0 | 0.0 | 0 | 1.17 | 1.25 |
| spa | crawl4ai-fit | 27 | 100 | 61.1 | 311 | 0.43 | 0.74 |
| spa | crawl4ai-raw | 27 | 100 | 100.0 | 393 | 0.43 | 0.90 |
| spa | scrapling | 27 | 100 | 38.9 | 75 | 0.20 | 0.40 |
| spa | trafilatura | 27 | 100 | 22.2 | 14 | 0.12 | 0.43 |
| spa | webclaw-auto | 27 | 100 | 77.8 | 323 | 1.72 | 2.17 |
| spa | webclaw-llm | 27 | 78 | 22.2 | 35 | 0.22 | 0.42 |
| spa | webclaw-md | 27 | 78 | 5.6 | 6 | 0.22 | 0.44 |
| spa | webclaw-render | 27 | 100 | 77.8 | 323 | 1.65 | 1.85 |

## html mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
| ALL | crawl4ai-fit | 69 | 97 | 73.4 | 1223 | 0.07 | 2.06 |
| ALL | crawl4ai-raw | 69 | 99 | 78.6 | 6756 | 0.04 | 1.03 |
| ALL | scrapling | 69 | 100 | 78.1 | 4363 | 0.02 | 0.49 |
| ALL | trafilatura | 69 | 100 | 64.6 | 625 | 0.02 | 0.33 |
| ALL | webclaw-llm | 69 | 100 | 75.0 | 1478 | 0.02 | 0.18 |
| ALL | webclaw-md | 69 | 97 | 73.4 | 2184 | 0.01 | 0.14 |
| docs | crawl4ai-fit | 9 | 100 | 100.0 | 7767 | 0.10 | 0.24 |
| docs | crawl4ai-raw | 9 | 100 | 100.0 | 12175 | 0.05 | 0.12 |
| docs | scrapling | 9 | 100 | 100.0 | 9895 | 0.04 | 0.05 |
| docs | trafilatura | 9 | 100 | 100.0 | 7865 | 0.02 | 0.04 |
| docs | webclaw-llm | 9 | 100 | 100.0 | 6429 | 0.02 | 0.04 |
| docs | webclaw-md | 9 | 100 | 100.0 | 7801 | 0.01 | 0.02 |
| ecommerce | crawl4ai-fit | 9 | 100 | 100.0 | 627 | 0.02 | 0.03 |
| ecommerce | crawl4ai-raw | 9 | 100 | 100.0 | 949 | 0.01 | 0.02 |
| ecommerce | scrapling | 9 | 100 | 100.0 | 956 | 0.00 | 0.01 |
| ecommerce | trafilatura | 9 | 100 | 100.0 | 581 | 0.00 | 0.01 |
| ecommerce | webclaw-llm | 9 | 100 | 100.0 | 967 | 0.01 | 0.01 |
| ecommerce | webclaw-md | 9 | 100 | 100.0 | 814 | 0.01 | 0.01 |
| homepage | crawl4ai-fit | 18 | 100 | 65.6 | 898 | 0.07 | 0.98 |
| homepage | crawl4ai-raw | 18 | 100 | 75.6 | 6099 | 0.07 | 0.30 |
| homepage | scrapling | 18 | 100 | 72.2 | 3514 | 0.02 | 0.04 |
| homepage | trafilatura | 18 | 100 | 46.7 | 355 | 0.02 | 0.06 |
| homepage | webclaw-llm | 18 | 100 | 68.9 | 1153 | 0.02 | 0.04 |
| homepage | webclaw-md | 18 | 100 | 68.9 | 2473 | 0.02 | 0.03 |
| news | crawl4ai-fit | 9 | 100 | 100.0 | 5004 | 0.10 | 1.04 |
| news | crawl4ai-raw | 9 | 100 | 100.0 | 10227 | 0.04 | 0.54 |
| news | scrapling | 9 | 100 | 100.0 | 10039 | 0.04 | 0.07 |
| news | trafilatura | 9 | 100 | 100.0 | 1756 | 0.02 | 0.08 |
| news | webclaw-llm | 9 | 100 | 100.0 | 3502 | 0.02 | 0.02 |
| news | webclaw-md | 9 | 100 | 100.0 | 3429 | 0.01 | 0.02 |
| nonenglish | crawl4ai-fit | 9 | 100 | 100.0 | 142484 | 1.83 | 27.26 |
| nonenglish | crawl4ai-raw | 9 | 100 | 100.0 | 208489 | 0.98 | 40.08 |
| nonenglish | scrapling | 9 | 100 | 100.0 | 202776 | 0.41 | 0.78 |
| nonenglish | trafilatura | 9 | 100 | 100.0 | 108598 | 0.31 | 0.67 |
| nonenglish | webclaw-llm | 9 | 100 | 100.0 | 64353 | 0.14 | 0.32 |
| nonenglish | webclaw-md | 9 | 100 | 100.0 | 148433 | 0.12 | 0.26 |
| paywalled | crawl4ai-fit | 6 | 100 | 50.0 | 504 | 0.01 | 0.07 |
| paywalled | crawl4ai-raw | 6 | 100 | 50.0 | 3364 | 0.01 | 0.21 |
| paywalled | scrapling | 6 | 100 | 50.0 | 3059 | 0.00 | 0.07 |
| paywalled | trafilatura | 6 | 100 | 50.0 | 190 | 0.00 | 0.03 |
| paywalled | webclaw-llm | 6 | 100 | 50.0 | 890 | 0.01 | 0.01 |
| paywalled | webclaw-md | 6 | 100 | 50.0 | 1081 | 0.01 | 0.01 |
| spa | crawl4ai-fit | 9 | 78 | 22.2 | 9 | 0.01 | 0.01 |
| spa | crawl4ai-raw | 9 | 89 | 27.8 | 70 | 0.01 | 0.01 |
| spa | scrapling | 9 | 100 | 38.9 | 75 | 0.00 | 0.00 |
| spa | trafilatura | 9 | 100 | 22.2 | 14 | 0.00 | 0.06 |
| spa | webclaw-llm | 9 | 100 | 22.2 | 25 | 0.01 | 0.01 |
| spa | webclaw-md | 9 | 78 | 5.6 | 6 | 0.01 | 0.01 |
