# Benchmark 2026-10-13-render-everywhere

webclaw: webclaw 0.6.24 +browser
Tool versions: [versions.txt](versions.txt)

Fact recall counts failed runs as 0 hits. Tokens: cl100k median over successful runs. Seconds: wall time per URL.

## url mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
| ALL | crawl4ai-fit | 234 | 85 | 70.6 | 1821 | 0.49 | 2.35 |
| ALL | crawl4ai-raw | 234 | 84 | 76.8 | 8422 | 0.43 | 1.20 |
| ALL | scrapling | 234 | 91 | 74.3 | 4825 | 0.27 | 0.80 |
| ALL | trafilatura | 234 | 75 | 52.4 | 1009 | 0.12 | 0.52 |
| ALL | webclaw-auto | 234 | 94 | 80.5 | 2761 | 0.27 | 1.85 |
| ALL | webclaw-llm | 234 | 91 | 75.7 | 2761 | 0.27 | 0.60 |
| ALL | webclaw-md | 234 | 91 | 74.3 | 4267 | 0.25 | 0.61 |
| ALL | webclaw-render | 234 | 78 | 71.9 | 2068 | 1.29 | 2.12 |
| docs | crawl4ai-fit | 27 | 100 | 100.0 | 7680 | 0.48 | 0.91 |
| docs | crawl4ai-raw | 27 | 100 | 100.0 | 11367 | 0.40 | 0.93 |
| docs | scrapling | 27 | 100 | 100.0 | 9895 | 0.12 | 0.34 |
| docs | trafilatura | 27 | 100 | 100.0 | 7865 | 0.07 | 0.32 |
| docs | webclaw-auto | 27 | 100 | 100.0 | 7282 | 0.12 | 0.35 |
| docs | webclaw-llm | 27 | 100 | 100.0 | 7282 | 0.10 | 0.70 |
| docs | webclaw-md | 27 | 100 | 100.0 | 8256 | 0.10 | 0.38 |
| docs | webclaw-render | 27 | 100 | 100.0 | 6738 | 1.00 | 1.56 |
| ecommerce | crawl4ai-fit | 27 | 100 | 100.0 | 651 | 0.32 | 0.95 |
| ecommerce | crawl4ai-raw | 27 | 100 | 100.0 | 1095 | 0.32 | 0.87 |
| ecommerce | scrapling | 27 | 100 | 100.0 | 956 | 0.39 | 0.52 |
| ecommerce | trafilatura | 27 | 100 | 100.0 | 581 | 0.12 | 0.16 |
| ecommerce | webclaw-auto | 27 | 100 | 100.0 | 1012 | 0.46 | 0.54 |
| ecommerce | webclaw-llm | 27 | 100 | 100.0 | 1012 | 0.44 | 0.53 |
| ecommerce | webclaw-md | 27 | 100 | 100.0 | 932 | 0.42 | 0.53 |
| ecommerce | webclaw-render | 27 | 100 | 100.0 | 1012 | 1.57 | 1.72 |
| homepage | crawl4ai-fit | 54 | 96 | 64.4 | 1332 | 0.65 | 1.45 |
| homepage | crawl4ai-raw | 54 | 96 | 72.6 | 5454 | 0.52 | 0.92 |
| homepage | scrapling | 54 | 100 | 77.8 | 3558 | 0.20 | 0.60 |
| homepage | trafilatura | 54 | 74 | 37.8 | 414 | 0.15 | 0.53 |
| homepage | webclaw-auto | 54 | 89 | 66.7 | 1374 | 0.21 | 1.04 |
| homepage | webclaw-llm | 54 | 89 | 66.7 | 1374 | 0.21 | 0.59 |
| homepage | webclaw-md | 54 | 89 | 66.7 | 2698 | 0.21 | 0.59 |
| homepage | webclaw-render | 54 | 89 | 72.2 | 1532 | 1.31 | 2.15 |
| news | crawl4ai-fit | 27 | 100 | 100.0 | 5134 | 0.46 | 0.85 |
| news | crawl4ai-raw | 27 | 100 | 100.0 | 10362 | 0.39 | 0.61 |
| news | scrapling | 27 | 100 | 100.0 | 10018 | 0.14 | 0.25 |
| news | trafilatura | 27 | 100 | 100.0 | 1756 | 0.05 | 0.50 |
| news | webclaw-auto | 27 | 100 | 100.0 | 3532 | 0.12 | 0.16 |
| news | webclaw-llm | 27 | 100 | 100.0 | 3532 | 0.13 | 0.59 |
| news | webclaw-md | 27 | 100 | 100.0 | 3429 | 0.12 | 0.58 |
| news | webclaw-render | 27 | 100 | 100.0 | 3532 | 1.31 | 2.44 |
| nonenglish | crawl4ai-fit | 27 | 100 | 100.0 | 145099 | 2.32 | 4.19 |
| nonenglish | crawl4ai-raw | 27 | 100 | 100.0 | 217962 | 1.06 | 1.65 |
| nonenglish | scrapling | 27 | 100 | 100.0 | 202776 | 0.58 | 1.35 |
| nonenglish | trafilatura | 27 | 100 | 100.0 | 108598 | 0.35 | 0.93 |
| nonenglish | webclaw-auto | 27 | 100 | 100.0 | 74007 | 0.36 | 0.59 |
| nonenglish | webclaw-llm | 27 | 100 | 100.0 | 74007 | 0.36 | 0.60 |
| nonenglish | webclaw-md | 27 | 100 | 100.0 | 167368 | 0.34 | 1.09 |
| nonenglish | webclaw-render | 27 | 100 | 100.0 | 73964 | 1.51 | 2.13 |
| paywalled | crawl4ai-fit | 18 | 61 | 61.1 | 1105 | 0.77 | 1.83 |
| paywalled | crawl4ai-raw | 18 | 50 | 50.0 | 8422 | 0.41 | 0.84 |
| paywalled | scrapling | 18 | 83 | 33.3 | 13 | 0.12 | 3.50 |
| paywalled | trafilatura | 18 | 0 | 0.0 | 0 | 0.11 | 0.15 |
| paywalled | webclaw-auto | 18 | 50 | 50.0 | 1803 | 0.12 | 0.65 |
| paywalled | webclaw-llm | 18 | 50 | 50.0 | 1803 | 0.12 | 0.80 |
| paywalled | webclaw-md | 18 | 50 | 50.0 | 2167 | 0.22 | 0.79 |
| paywalled | webclaw-render | 18 | 0 | 0.0 | 0 | 1.05 | 1.24 |
| pdf | crawl4ai-fit | 27 | 0 | 0.0 | 0 | 0.06 | 0.12 |
| pdf | crawl4ai-raw | 27 | 0 | 0.0 | 0 | 0.06 | 0.12 |
| pdf | scrapling | 27 | 33 | 16.7 | 304113 | 0.29 | 0.41 |
| pdf | trafilatura | 27 | 0 | 0.0 | 0 | 0.08 | 0.46 |
| pdf | webclaw-auto | 27 | 100 | 94.4 | 17043 | 0.27 | 0.54 |
| pdf | webclaw-llm | 27 | 100 | 94.4 | 17043 | 0.30 | 0.59 |
| pdf | webclaw-md | 27 | 100 | 94.4 | 17691 | 0.26 | 0.49 |
| pdf | webclaw-render | 27 | 0 | 0.0 | 0 | 1.05 | 1.18 |
| spa | crawl4ai-fit | 27 | 100 | 61.1 | 311 | 0.44 | 0.86 |
| spa | crawl4ai-raw | 27 | 100 | 100.0 | 393 | 0.44 | 1.48 |
| spa | scrapling | 27 | 100 | 38.9 | 75 | 0.31 | 0.40 |
| spa | trafilatura | 27 | 100 | 22.2 | 14 | 0.13 | 0.34 |
| spa | webclaw-auto | 27 | 100 | 77.8 | 323 | 1.58 | 1.98 |
| spa | webclaw-llm | 27 | 78 | 22.2 | 35 | 0.22 | 0.42 |
| spa | webclaw-md | 27 | 78 | 5.6 | 6 | 0.22 | 0.41 |
| spa | webclaw-render | 27 | 100 | 77.8 | 323 | 1.52 | 2.14 |

## html mode

| category | arm | runs | ok % | fact recall % | median tokens | p50 s | p95 s |
|---|---|---:|---:|---:|---:|---:|---:|
| ALL | crawl4ai-fit | 69 | 97 | 73.4 | 1223 | 0.09 | 2.24 |
| ALL | crawl4ai-raw | 69 | 99 | 78.6 | 6756 | 0.04 | 1.07 |
| ALL | scrapling | 69 | 100 | 78.1 | 4363 | 0.02 | 0.40 |
| ALL | trafilatura | 69 | 100 | 64.6 | 625 | 0.01 | 0.33 |
| ALL | webclaw-llm | 69 | 100 | 75.0 | 1478 | 0.02 | 0.17 |
| ALL | webclaw-md | 69 | 97 | 73.4 | 2184 | 0.01 | 0.11 |
| docs | crawl4ai-fit | 9 | 100 | 100.0 | 7767 | 0.21 | 0.58 |
| docs | crawl4ai-raw | 9 | 100 | 100.0 | 12175 | 0.04 | 0.11 |
| docs | scrapling | 9 | 100 | 100.0 | 9895 | 0.04 | 0.06 |
| docs | trafilatura | 9 | 100 | 100.0 | 7865 | 0.02 | 0.04 |
| docs | webclaw-llm | 9 | 100 | 100.0 | 6429 | 0.02 | 0.04 |
| docs | webclaw-md | 9 | 100 | 100.0 | 7801 | 0.01 | 0.03 |
| ecommerce | crawl4ai-fit | 9 | 100 | 100.0 | 627 | 0.02 | 0.03 |
| ecommerce | crawl4ai-raw | 9 | 100 | 100.0 | 949 | 0.01 | 0.02 |
| ecommerce | scrapling | 9 | 100 | 100.0 | 956 | 0.01 | 0.01 |
| ecommerce | trafilatura | 9 | 100 | 100.0 | 581 | 0.00 | 0.00 |
| ecommerce | webclaw-llm | 9 | 100 | 100.0 | 967 | 0.01 | 0.02 |
| ecommerce | webclaw-md | 9 | 100 | 100.0 | 814 | 0.01 | 0.01 |
| homepage | crawl4ai-fit | 18 | 100 | 65.6 | 898 | 0.10 | 0.98 |
| homepage | crawl4ai-raw | 18 | 100 | 75.6 | 6099 | 0.05 | 0.32 |
| homepage | scrapling | 18 | 100 | 72.2 | 3514 | 0.02 | 0.06 |
| homepage | trafilatura | 18 | 100 | 46.7 | 355 | 0.02 | 0.07 |
| homepage | webclaw-llm | 18 | 100 | 68.9 | 1153 | 0.02 | 0.05 |
| homepage | webclaw-md | 18 | 100 | 68.9 | 2473 | 0.02 | 0.04 |
| news | crawl4ai-fit | 9 | 100 | 100.0 | 5004 | 0.10 | 0.96 |
| news | crawl4ai-raw | 9 | 100 | 100.0 | 10227 | 0.04 | 0.54 |
| news | scrapling | 9 | 100 | 100.0 | 10039 | 0.04 | 0.10 |
| news | trafilatura | 9 | 100 | 100.0 | 1756 | 0.02 | 0.09 |
| news | webclaw-llm | 9 | 100 | 100.0 | 3502 | 0.02 | 0.03 |
| news | webclaw-md | 9 | 100 | 100.0 | 3429 | 0.01 | 0.02 |
| nonenglish | crawl4ai-fit | 9 | 100 | 100.0 | 142484 | 1.95 | 21.67 |
| nonenglish | crawl4ai-raw | 9 | 100 | 100.0 | 208489 | 1.01 | 54.74 |
| nonenglish | scrapling | 9 | 100 | 100.0 | 202776 | 0.39 | 0.79 |
| nonenglish | trafilatura | 9 | 100 | 100.0 | 108598 | 0.29 | 0.71 |
| nonenglish | webclaw-llm | 9 | 100 | 100.0 | 64353 | 0.16 | 0.33 |
| nonenglish | webclaw-md | 9 | 100 | 100.0 | 148433 | 0.10 | 0.24 |
| paywalled | crawl4ai-fit | 6 | 100 | 50.0 | 504 | 0.01 | 0.09 |
| paywalled | crawl4ai-raw | 6 | 100 | 50.0 | 3364 | 0.01 | 0.10 |
| paywalled | scrapling | 6 | 100 | 50.0 | 3059 | 0.00 | 0.02 |
| paywalled | trafilatura | 6 | 100 | 50.0 | 190 | 0.01 | 0.03 |
| paywalled | webclaw-llm | 6 | 100 | 50.0 | 890 | 0.01 | 0.03 |
| paywalled | webclaw-md | 6 | 100 | 50.0 | 1081 | 0.01 | 0.01 |
| spa | crawl4ai-fit | 9 | 78 | 22.2 | 9 | 0.01 | 0.03 |
| spa | crawl4ai-raw | 9 | 89 | 27.8 | 70 | 0.01 | 0.01 |
| spa | scrapling | 9 | 100 | 38.9 | 75 | 0.00 | 0.00 |
| spa | trafilatura | 9 | 100 | 22.2 | 14 | 0.00 | 0.09 |
| spa | webclaw-llm | 9 | 100 | 22.2 | 25 | 0.01 | 0.01 |
| spa | webclaw-md | 9 | 78 | 5.6 | 6 | 0.01 | 0.02 |
