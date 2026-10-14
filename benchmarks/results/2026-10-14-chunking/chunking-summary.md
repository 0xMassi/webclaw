# Chunking for retrieval 2026-10-14-chunking

69 pages frozen by `run.sh`, 40 questions from `questions.jsonl`. Sizes are cl100k tokens: `±25%` is the share of chunks within a quarter of the target, `over` the share above 125%. `sentences whole`: prose sentences of 8 to 60 words in the markdown the arm chunked that sit inside one chunk (links reduced to their text, whitespace squashed). `mid-sentence`: chunks other than a page's last that do not end on `.`, `!`, `?`, `:`, `|`, a closing fence or `)`. Facts: golden case facts found in that markdown; `whole` counts those inside one chunk. Retrieval: one BM25 scorer (k1 1.2, b 0.75, lowercase words, CJK character pairs, no stemming) ranks each page's chunks against the question; a hit is a chunk holding an answer. `within 512` and `within 1024` read ranked chunks in order until that many cl100k tokens and count a hit found inside the budget. Seconds per page run from the frozen HTML: markdown plus chunking. The cl100k fixed windows cut webclaw's markdown every N tokens of the real tokenizer, at the target and at the median size webclaw's chunks come out at. Crawl4AI's window sizes are words: the target over its own 1.3 tokens-per-word rate. `regex + merge` is its LLM extraction path (paragraph split, then `merge_chunks` to the token target with 10% overlap). Scrapling has no chunker; its row cuts its markdown every 4 characters per target token, the split a user would write.

## target 256 tokens

| arm | chunks | median tokens | p95 tokens | ±25% | over | sentences whole | mid-sentence | facts whole | top-1 | top-3 | within 512 | within 1024 | p50 s/page |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw --chunk | 9012 | 185 | 255 | 45% | 0% | 98% | 23% | 141/141 | 31/40 | 37/40 | 36/40 | 40/40 | 0.020 |
| webclaw --chunk --chunk-overlap 25 | 9366 | 180 | 246 | 40% | 0% | 98% | 24% | 141/141 | 31/40 | 38/40 | 36/40 | 40/40 | 0.015 |
| crawl4ai sliding window | 9736 | 1086 | 6170 | 4% | 95% | 100% | 91% | 151/151 | 31/40 | 37/40 | 23/40 | 32/40 | 0.039 |
| crawl4ai overlapping window | 5401 | 1079 | 6133 | 5% | 95% | 97% | 91% | 151/151 | 31/40 | 37/40 | 22/40 | 32/40 | 0.039 |
| crawl4ai regex | 3715 | 88 | 3280 | 10% | 23% | 100% | 33% | 151/151 | 35/40 | 38/40 | 12/40 | 20/40 | 0.039 |
| crawl4ai regex + merge | 4174 | 1405 | 8169 | 1% | 99% | 98% | 92% | 151/151 | 36/40 | 39/40 | 23/40 | 31/40 | 0.039 |
| crawl4ai sliding window on webclaw markdown | 3375 | 752 | 2258 | 13% | 86% | 100% | 83% | 141/141 | 34/40 | 37/40 | 23/40 | 33/40 | 0.014 |
| crawl4ai overlapping window on webclaw markdown | 1904 | 741 | 2327 | 14% | 85% | 98% | 82% | 141/141 | 35/40 | 38/40 | 23/40 | 33/40 | 0.014 |
| crawl4ai regex on webclaw markdown | 9863 | 44 | 522 | 8% | 11% | 100% | 37% | 141/141 | 23/40 | 32/40 | 31/40 | 36/40 | 0.014 |
| crawl4ai regex + merge on webclaw markdown | 1473 | 979 | 2886 | 2% | 97% | 99% | 83% | 141/141 | 34/40 | 37/40 | 21/40 | 31/40 | 0.014 |
| scrapling (no chunker): fixed 4-chars-per-token split | 6082 | 332 | 727 | 38% | 61% | 76% | 95% | 149/150 | 29/40 | 39/40 | 31/40 | 38/40 | 0.021 |
| cl100k fixed windows on webclaw markdown, at the target | 6227 | 256 | 256 | 99% | 0% | 74% | 95% | 141/141 | 28/40 | 34/40 | 34/40 | 38/40 | 0.014 |
| cl100k fixed windows on webclaw markdown, at webclaw's median, 185 | 8596 | 185 | 185 | 0% | 0% | 64% | 95% | 141/141 | 32/40 | 37/40 | 34/40 | 39/40 | 0.014 |
| webclaw --chunk, heading path scored with the text | 9012 | 195 | 265 | 51% | 1% | 98% | 23% | 141/141 | 31/40 | 39/40 | 36/40 | 40/40 | 0.020 |

## target 512 tokens

| arm | chunks | median tokens | p95 tokens | ±25% | over | sentences whole | mid-sentence | facts whole | top-1 | top-3 | within 512 | within 1024 | p50 s/page |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| webclaw --chunk | 4321 | 387 | 507 | 51% | 0% | 100% | 21% | 141/141 | 34/40 | 39/40 | 34/40 | 37/40 | 0.015 |
| webclaw --chunk --chunk-overlap 51 | 4528 | 378 | 494 | 47% | 0% | 100% | 20% | 141/141 | 33/40 | 40/40 | 33/40 | 39/40 | 0.015 |
| crawl4ai sliding window | 4837 | 2213 | 12286 | 4% | 96% | 100% | 92% | 151/151 | 36/40 | 38/40 | 8/40 | 25/40 | 0.039 |
| crawl4ai overlapping window | 2722 | 2181 | 12033 | 4% | 95% | 100% | 92% | 151/151 | 34/40 | 38/40 | 6/40 | 24/40 | 0.039 |
| crawl4ai regex | 3715 | 88 | 3280 | 6% | 14% | 100% | 33% | 151/151 | 35/40 | 38/40 | 12/40 | 20/40 | 0.039 |
| crawl4ai regex + merge | 2107 | 2880 | 15744 | 1% | 98% | 100% | 91% | 151/151 | 38/40 | 38/40 | 1/40 | 24/40 | 0.039 |
| crawl4ai sliding window on webclaw markdown | 1673 | 1535 | 4440 | 12% | 87% | 100% | 83% | 141/141 | 35/40 | 38/40 | 4/40 | 25/40 | 0.014 |
| crawl4ai overlapping window on webclaw markdown | 969 | 1508 | 4426 | 13% | 84% | 100% | 83% | 141/141 | 35/40 | 39/40 | 6/40 | 26/40 | 0.014 |
| crawl4ai regex on webclaw markdown | 9863 | 44 | 522 | 5% | 3% | 100% | 37% | 141/141 | 23/40 | 32/40 | 31/40 | 36/40 | 0.014 |
| crawl4ai regex + merge on webclaw markdown | 754 | 1947 | 5429 | 2% | 95% | 100% | 83% | 141/141 | 34/40 | 38/40 | 1/40 | 23/40 | 0.014 |
| scrapling (no chunker): fixed 4-chars-per-token split | 3063 | 662 | 1452 | 38% | 60% | 88% | 94% | 149/150 | 33/40 | 40/40 | 19/40 | 32/40 | 0.021 |
| cl100k fixed windows on webclaw markdown, at the target | 3130 | 512 | 512 | 98% | 0% | 87% | 95% | 141/141 | 33/40 | 39/40 | 33/40 | 37/40 | 0.014 |
| cl100k fixed windows on webclaw markdown, at webclaw's median, 387 | 4132 | 387 | 387 | 98% | 0% | 82% | 94% | 141/141 | 32/40 | 39/40 | 32/40 | 36/40 | 0.014 |
| webclaw --chunk, heading path scored with the text | 4321 | 396 | 517 | 55% | 0% | 100% | 21% | 141/141 | 34/40 | 39/40 | 33/40 | 38/40 | 0.015 |

webclaw's `tokens` estimate against cl100k over 13333 chunks: cl100k / estimate p5 0.77, p50 0.91, p95 1.12.

Every answer sits in some chunk for every arm.

Pages with a golden fact split across chunks: scrapling (no chunker): fixed 4-chars-per-token split @256: home06; scrapling (no chunker): fixed 4-chars-per-token split @512: home06
