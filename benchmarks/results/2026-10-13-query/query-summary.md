# Query-focused extraction 2026-10-13-query

40 questions over 28 frozen pages (`questions.jsonl`). Each question was written apart from its answers: no answer string appears in its question. Answer recall counts answers found in the output, with failed runs as misses; `ok` counts runs that returned text without an error. Tokens: cl100k, median over ok runs. The `cut to` rows truncate the tool's full output to the token count its own query filter returned for the same question, so a filter has to beat keeping the top of the page.

## all questions

| arm | questions | ok | answer recall % | median tokens | p50 s |
|---|---:|---:|---:|---:|---:|
| webclaw-llm | 40 | 40 | 100.0 | 7022 | 0.02 |
| webclaw-query | 40 | 40 | 92.7 | 781 | 0.02 |
| scrapling | 40 | 40 | 100.0 | 11332 | 0.04 |
| crawl4ai-raw | 40 | 40 | 100.0 | 19096 | 0.04 |
| crawl4ai-bm25 | 40 | 40 | 85.4 | 1765 | 0.11 |
| webclaw-llm cut to webclaw-query size | 40 | 40 | 56.1 | 781 | 0.00 |
| crawl4ai-raw cut to crawl4ai-bm25 size | 40 | 40 | 31.7 | 1765 | 0.00 |

## English pages

| arm | questions | ok | answer recall % | median tokens | p50 s |
|---|---:|---:|---:|---:|---:|
| webclaw-llm | 30 | 30 | 100.0 | 5697 | 0.02 |
| webclaw-query | 30 | 30 | 96.8 | 674 | 0.02 |
| scrapling | 30 | 30 | 100.0 | 9688 | 0.03 |
| crawl4ai-raw | 30 | 30 | 100.0 | 11539 | 0.04 |
| crawl4ai-bm25 | 30 | 30 | 87.1 | 1661 | 0.10 |
| webclaw-llm cut to webclaw-query size | 30 | 30 | 48.4 | 674 | 0.00 |
| crawl4ai-raw cut to crawl4ai-bm25 size | 30 | 30 | 32.3 | 1661 | 0.00 |

## non-English pages

| arm | questions | ok | answer recall % | median tokens | p50 s |
|---|---:|---:|---:|---:|---:|
| webclaw-llm | 10 | 10 | 100.0 | 70668 | 0.14 |
| webclaw-query | 10 | 10 | 80.0 | 1499 | 0.11 |
| scrapling | 10 | 10 | 100.0 | 199519 | 0.35 |
| crawl4ai-raw | 10 | 10 | 100.0 | 204386 | 0.88 |
| crawl4ai-bm25 | 10 | 10 | 80.0 | 6141 | 1.18 |
| webclaw-llm cut to webclaw-query size | 10 | 10 | 80.0 | 1499 | 0.00 |
| crawl4ai-raw cut to crawl4ai-bm25 size | 10 | 10 | 30.0 | 6141 | 0.00 |
