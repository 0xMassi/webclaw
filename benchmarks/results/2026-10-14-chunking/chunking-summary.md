# Chunking for retrieval 2026-10-14-chunking

69 pages frozen by `run.sh`, 40 questions from `questions.jsonl`. Facts: golden case facts found in the markdown the arm chunked; `whole` counts those that sit inside one chunk. Retrieval: one BM25 scorer (k1 1.2, b 0.75, lowercase words, CJK character pairs, no stemming) ranks each page's chunks against the question; a hit is a chunk holding an answer. `reachable` is any chunk holding it. Sizes are cl100k tokens. `±25%` is the share of chunks within a quarter of the target; `over` the share above 125%. Seconds per page run from the frozen HTML: markdown plus chunking. Crawl4AI's window sizes are words: the target over its own 1.3 tokens-per-word rate. `regex + merge` is its LLM extraction path (paragraph split, then `merge_chunks` to the token target with 10% overlap). Scrapling has no chunker; its row cuts its markdown every 4 characters per target token, the split a user would write.

## target 256 tokens

| arm | chunks | median tokens | p95 tokens | ±25% | over | facts whole | top-1 | top-3 | reachable | top-3 tokens | p50 s/page |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw --chunk | 8772 | 187 | 261 | 46% | 1% | 141/141 | 31/40 | 38/40 | 40/40 | 547 | 0.019 |
| webclaw --chunk --chunk-overlap 25 | 9070 | 183 | 254 | 42% | 0% | 141/141 | 30/40 | 38/40 | 40/40 | 544 | 0.023 |
| crawl4ai sliding window | 9736 | 1086 | 6170 | 4% | 95% | 151/151 | 31/40 | 37/40 | 40/40 | 1176 | 0.045 |
| crawl4ai overlapping window | 5401 | 1079 | 6133 | 5% | 95% | 151/151 | 31/40 | 37/40 | 40/40 | 1260 | 0.045 |
| crawl4ai regex | 3715 | 88 | 3280 | 10% | 23% | 151/151 | 35/40 | 38/40 | 40/40 | 4311 | 0.045 |
| crawl4ai regex + merge | 4174 | 1405 | 8169 | 1% | 99% | 151/151 | 36/40 | 39/40 | 40/40 | 1524 | 0.045 |
| crawl4ai sliding window on webclaw markdown | 3375 | 752 | 2258 | 13% | 86% | 141/141 | 34/40 | 37/40 | 40/40 | 995 | 0.016 |
| crawl4ai overlapping window on webclaw markdown | 1904 | 741 | 2327 | 14% | 85% | 141/141 | 35/40 | 38/40 | 40/40 | 982 | 0.016 |
| crawl4ai regex on webclaw markdown | 9863 | 44 | 522 | 8% | 11% | 141/141 | 23/40 | 32/40 | 40/40 | 267 | 0.016 |
| crawl4ai regex + merge on webclaw markdown | 1473 | 979 | 2886 | 2% | 97% | 141/141 | 34/40 | 37/40 | 40/40 | 1313 | 0.016 |
| scrapling (no chunker): fixed 4-chars-per-token split | 6082 | 332 | 727 | 38% | 61% | 149/150 | 29/40 | 39/40 | 40/40 | 771 | 0.021 |
| webclaw --chunk, heading path scored with the text | 8772 | 196 | 272 | 52% | 1% | 141/141 | 31/40 | 39/40 | 40/40 | 578 | 0.019 |

## target 512 tokens

| arm | chunks | median tokens | p95 tokens | ±25% | over | facts whole | top-1 | top-3 | reachable | top-3 tokens | p50 s/page |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw --chunk | 4237 | 393 | 519 | 53% | 0% | 141/141 | 34/40 | 39/40 | 40/40 | 1177 | 0.024 |
| webclaw --chunk --chunk-overlap 51 | 4436 | 381 | 504 | 48% | 0% | 141/141 | 32/40 | 39/40 | 40/40 | 1137 | 0.015 |
| crawl4ai sliding window | 4837 | 2213 | 12286 | 4% | 96% | 151/151 | 36/40 | 38/40 | 40/40 | 2268 | 0.045 |
| crawl4ai overlapping window | 2722 | 2181 | 12033 | 4% | 95% | 151/151 | 34/40 | 38/40 | 40/40 | 2475 | 0.045 |
| crawl4ai regex | 3715 | 88 | 3280 | 6% | 14% | 151/151 | 35/40 | 38/40 | 40/40 | 4311 | 0.045 |
| crawl4ai regex + merge | 2107 | 2880 | 15744 | 1% | 98% | 151/151 | 38/40 | 38/40 | 40/40 | 3471 | 0.045 |
| crawl4ai sliding window on webclaw markdown | 1673 | 1535 | 4440 | 12% | 87% | 141/141 | 35/40 | 38/40 | 40/40 | 2065 | 0.016 |
| crawl4ai overlapping window on webclaw markdown | 969 | 1508 | 4426 | 13% | 84% | 141/141 | 35/40 | 39/40 | 40/40 | 2013 | 0.016 |
| crawl4ai regex on webclaw markdown | 9863 | 44 | 522 | 5% | 3% | 141/141 | 23/40 | 32/40 | 40/40 | 267 | 0.016 |
| crawl4ai regex + merge on webclaw markdown | 754 | 1947 | 5429 | 2% | 95% | 141/141 | 34/40 | 38/40 | 40/40 | 2570 | 0.016 |
| scrapling (no chunker): fixed 4-chars-per-token split | 3063 | 662 | 1452 | 38% | 60% | 149/150 | 33/40 | 40/40 | 40/40 | 1522 | 0.021 |
| webclaw --chunk, heading path scored with the text | 4237 | 402 | 529 | 57% | 1% | 141/141 | 34/40 | 39/40 | 40/40 | 1214 | 0.024 |

webclaw's `tokens` estimate against cl100k over 13009 chunks: cl100k / estimate p5 0.77, p50 0.92, p95 1.15.

Pages with a golden fact split across chunks: scrapling (no chunker): fixed 4-chars-per-token split @256: home06; scrapling (no chunker): fixed 4-chars-per-token split @512: home06
