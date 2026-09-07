# Offline extraction benchmark

This benchmark isolates local extraction CPU work from fetching, rendering, and
LLM calls. It uses the tracked news and Reddit HTML captures plus deterministic
synthetic article and catalog pages. It adds no dependencies.

Build the example on the baseline source, preserve the binary, then repeat on
the candidate with the same toolchain, features, and release settings:

```sh
cargo build --locked --release -p webclaw-core --example extraction_bench
cp target/release/examples/extraction_bench /tmp/extraction-before
# Apply the candidate change, then rebuild.
cargo build --locked --release -p webclaw-core --example extraction_bench
cp target/release/examples/extraction_bench /tmp/extraction-after
python3 benchmarks/scripts/extraction_fixtures.py /tmp/extraction-fixtures
python3 benchmarks/scripts/compare_extraction.py \
  /tmp/extraction-before /tmp/extraction-after \
  /tmp/extraction-fixtures/manifest.json /tmp/extraction-results
```

When benchmarking older revisions, copy this example and scripts into the
baseline checkout first. They must be identical in both builds. Keep input
files, package features, compiler flags, and toolchain unchanged. Run on an idle
machine without concurrent builds or profilers; retain the revision, source
diff, toolchain/OS/hardware details, and generated fixture hashes with results.

The comparison first checks complete extraction JSON for each fixture under
four option sets: default, main content, raw HTML, and body inclusion with
navigation/footer exclusion. Then it measures `extract` and `jsonld` in seven
alternating before/after rounds, with 100 operations per process after five
warmups. Input loading and output serialization are outside timing; result
allocation, destruction, and extraction's normal worker thread are included.
`--rounds` and `--iterations` adjust repetition. All raw samples are retained.

`summary.json` reports the median of round medians, pooled per-operation p95,
and paired percent changes (positive means faster) including their range.
Percent changes use each round's before/after medians, so they can differ from
the percentage computed from the final two aggregate medians. These timings
are local latency measurements, not API throughput or production percentiles.

For a single component, use the example directly:

```sh
/tmp/extraction-before parse crates/webclaw-core/testdata/express_test.html \
  https://example.com/page 100 /tmp/parse.json
/tmp/extraction-before content crates/webclaw-core/testdata/express_test.html \
  https://example.com/page 100 /tmp/content.json
```

`parse` includes HTML parsing and destruction. `content` starts with an already
parsed document and measures readability scoring and markdown conversion.
These components are diagnostic and do not sum to full extraction: the full
pipeline also handles retries, metadata, structured data, and domain-specific
paths. In particular, Reddit bypasses generic content scoring.

The synthetic catalogs vary JSON-LD block count (0, 1, 10, 100) ahead of a
525 KB inline-script tail to expose repeated suffix scans. The unterminated
script workload checks the cost of a missing closing tag. Neither establishes
the frequency or distribution of those shapes in production. The pure article
and Reddit captures serve as controls for flows without JSON-LD.

Timing does not establish correctness. Run the crate's tests and WASM checks,
and keep focused regression assertions for any parser behavior being changed.
Process peak RSS can be compared separately with `/usr/bin/time -l` on macOS;
run it outside latency measurements and repeat it, since allocator retention
and runtime overhead affect this coarse metric. Cold initialization and
concurrent server throughput require separate workloads.
