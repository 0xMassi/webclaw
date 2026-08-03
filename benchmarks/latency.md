# Fetch-path latency benchmark

`crates/webclaw-fetch/examples/latency_bench.rs` measures where wall-clock time
actually goes on the fetch path, separating **network** from **CPU**:

- `fetch_ms` — `FetchClient::fetch`: DNS + TCP + TLS + request + TTFB + body.
- `extract_ms` — `webclaw_core::extract_with_options`: pure parse + extraction.

This is deliberately different from `benchmarks/methodology.md`, which measures
extraction *quality* (tokens and fidelity). This one measures *speed*, and its
job is to say which half is worth optimising before anyone optimises anything.

## Running

```bash
cargo build --release -p webclaw-fetch --example latency_bench

BENCH_URLS=/path/to/urls.txt \
BENCH_CONCURRENCY=8 \
BENCH_LIMIT=200 \
  ./target/release/examples/latency_bench > run.jsonl 2> run.summary
```

| Env | Default | Meaning |
|---|---|---|
| `BENCH_URLS` | *(required)* | File of URLs, one per line |
| `BENCH_CONCURRENCY` | 8 | In-flight requests |
| `BENCH_LIMIT` | all | Cap URLs read |
| `BENCH_TIMEOUT_SECS` | 12 | Per-request timeout |
| `WEBCLAW_PROXY` | unset | Optional proxy |

The URL list is passed in at runtime and is **never committed** — a realistic
corpus is drawn from whatever traffic you actually serve, which is not public
data. Lines may carry extra `|`-separated columns (only the first is read), so a
CSV/DB export can be fed in unchanged; `#` comments and blanks are skipped.

stdout is JSONL (one object per URL: status, timings, bytes, words, error) so
runs can be diffed over time. stderr carries the percentile summary.

## Interpreting a run

The summary prints `p50/p90/p99/max` for fetch, extract, and total, plus:

- **`extract share of measured time`** — if this is low single digits, the
  extractor is not the bottleneck and parser micro-optimisation is wasted
  effort. Attack the network path instead.
- **`200s that would trip the thin-body escalation rule`** — pages returning
  HTTP 200 with under 10 KB of HTML *or* under 50 extracted words. Downstream
  consumers treat these as "not the real page" and re-fetch them through a
  browser, which costs an order of magnitude more than the direct fetch. This
  count is the size of that exposure *before* any escalation runs.

That last number is worth splitting by cause when it looks high. A page tripping
on **word count with a large HTML body** is a client-rendered shell — the static
HTML genuinely has no content, and a browser render is the correct answer. A
page tripping on **byte size while carrying real words** would be a false
positive. The two demand opposite fixes, so measure before concluding.

## Reading the tail

Latency here is heavily tail-weighted: on a representative corpus a low
single-digit percentage of URLs can account for roughly a third of total fetch
time, and `p99` tends to sit at whatever `BENCH_TIMEOUT_SECS` is set to — that
is the timeout wall, not a measurement. Compare `p50` for typical cost and count
requests at the timeout separately; averaging across both hides each.
