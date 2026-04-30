# Changelog

All notable changes to webclaw are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/).

## [0.5.7] — 2026-04-30

### Docs
- README header now uses an `<h1>webclaw</h1>` instead of an `<h3>` slogan. The repo had no heading-level brand anchor before, only a banner image, so search engines indexing the README were missing the canonical brand signal. The new heading is what GitHub renders as the title of the page and what Google co-ranks with webclaw.io.

---

## [0.5.6] — 2026-04-23

### Added
- `FetchClient::fetch_smart(url)` applies per-site rescue logic and returns the same `FetchResult` shape as `fetch()`. Reddit URLs route to the `.json` API with an identifiable bot `User-Agent`, and Akamai-style challenge pages trigger a homepage cookie warmup plus a retry. Makes `/v1/scrape` on Reddit populate markdown again.

### Fixed
- Regression introduced in 0.5.4 where the production server's `/v1/scrape` bypassed the Reddit `.json` shortcut and Akamai cookie warmup that `fetch_and_extract` had been providing. Both helpers now live in `fetch_smart` and every caller path picks them up.
- Panic in the markdown converter (`markdown.rs:925`) on single-pipe `|` lines. A `[1..len-1]` slice on a 1-char input triggered `begin <= end`. Guarded.

---

## [0.5.5] — 2026-04-23

### Added
- `webclaw --browser safari-ios` on the CLI. Pairs with `--proxy` for DataDome-fronted sites that reject desktop profiles.

---

## [0.5.4] — 2026-04-23

### Added
- New `BrowserProfile::SafariIos` for Safari iOS 26 fingerprinting. Pairs with a country-matched residential proxy for sites that reject non-mobile profiles.
- `accept_language_for_url(url)` and `accept_language_for_tld(tld)` helpers. Returns a locale-appropriate `Accept-Language` based on the URL's TLD, with `en-US` as the fallback.

### Changed
- Chrome browser fingerprint refreshed for current Cloudflare bot management. Fixes 403 challenges on several e-commerce and jobs sites.
- Bumped `wreq-util` to `3.0.0-rc.10`.

---

## [0.5.2] — 2026-04-22

### Added
- **`webclaw vertical <name> <url>` subcommand on the CLI.** Runs a specific vertical extractor and prints typed JSON (pretty-printed by default, `--raw` for single-line). Example: `webclaw vertical reddit https://www.reddit.com/r/rust/comments/abc/` returns `{post: {title, author, points, ...}, comments: [...]}`. URL-mismatch errors surface cleanly as `"URL '...' does not match the '...' extractor"` on stderr with exit code 1.

- **`webclaw extractors` subcommand on the CLI.** Lists all 28 vertical extractors with name, label, and one URL pattern sample. `--json` emits the full catalog as JSON (same shape as `GET /v1/extractors`) for tooling. Covers discovery for users who don't know which vertical to pick.

- **`vertical_scrape` and `list_extractors` tools on `webclaw-mcp`.** Claude Desktop / Claude Code users can now call any of the 28 extractors by name from an MCP session. Tool count goes from 10 to 12. `list_extractors` takes no args and returns the full catalog; `vertical_scrape` takes `{name, url}` and returns the typed JSON payload. Antibot-gated verticals still auto-escalate to the webclaw cloud API when `WEBCLAW_API_KEY` is set.

### Changed
- Server-info instruction string in `webclaw-mcp` now lists all 12 tools (previously hard-coded 10). Also `webclaw --help` on the CLI now shows the three subcommands: `bench`, `extractors`, `vertical`.

---

## [0.5.1] — 2026-04-22

### Added
- **`webclaw_fetch::Fetcher` trait.** Vertical extractors now consume `&dyn Fetcher` instead of `&FetchClient` directly. The trait exposes three methods (`fetch`, `fetch_with_headers`, `cloud`) covering everything extractors need. Callers that already held a `FetchClient` keep working unchanged: `FetchClient` implements `Fetcher`, blanket impls cover `&T` and `Arc<T>`, so `&client` coerces to `&dyn Fetcher` automatically.

  The motivation is the split between OSS (wreq-backed, in-process TLS fingerprinting) and the production API server at api.webclaw.io (which cannot use in-process fingerprinting per the architecture rule, and must delegate HTTP through the Go tls-sidecar). Before this trait, adding vertical routes to the production server would have required importing wreq into its dependency graph, violating the separation. Now the production server can provide its own `TlsSidecarFetcher` implementation and pass it to the same extractor dispatcher the OSS server uses.

  Backwards compatible. No behavior change for CLI, MCP, or OSS self-host.

### Changed
- All 28 extractor `extract()` signatures migrated from `client: &FetchClient` to `client: &dyn Fetcher`. The dispatcher functions (`extractors::dispatch_by_url`, `extractors::dispatch_by_name`) and the cloud escalation helpers (`cloud::smart_fetch`, `cloud::smart_fetch_html`) follow the same change. Tests and call sites are unchanged because `&FetchClient` auto-coerces.

---

## [0.5.0] — 2026-04-22

### Added
- **28 vertical extractors that return typed JSON instead of generic markdown.** New `webclaw_fetch::extractors` module with one extractor per site. Dev: reddit, hackernews, github_repo / github_pr / github_issue / github_release, crates_io, pypi, npm. AI/ML: huggingface_model, huggingface_dataset, arxiv, docker_hub. Writing: dev_to, stackoverflow, youtube_video. Social: linkedin_post, instagram_post, instagram_profile. Ecommerce: shopify_product, shopify_collection, ecommerce_product (generic Schema.org), woocommerce_product, amazon_product, ebay_listing, etsy_listing. Reviews: trustpilot_reviews, substack_post. Each extractor claims a URL pattern via a public `matches()` fn and returns a typed JSON payload with the fields callers actually want (title, price, author, rating, review count, etc.) rather than a markdown blob.
- **`POST /v1/scrape/{vertical}` on `webclaw-server` for explicit vertical routing.** Picks the parser by name, validates the URL plausibly belongs to that vertical, returns the same shape as `POST /v1/scrape` but typed. 23 of 28 verticals also auto-dispatch from a plain `POST /v1/scrape` because their URL shapes are unique enough to claim safely; the remaining 5 (`shopify_product`, `shopify_collection`, `ecommerce_product`, `woocommerce_product`, `substack_post`) use patterns that non-target sites share, so callers opt in via the `{vertical}` route.
- **`GET /v1/extractors` on `webclaw-server`.** Returns the full catalog as `{"extractors": [{"name": "...", "label": "...", "description": "...", "url_patterns": [...]}, ...]}` so clients can build tooling / autocomplete / user-facing docs off a live source.
- **Antibot cloud-escalation for 5 ecommerce + reviews verticals.** Amazon, eBay, Etsy, Trustpilot, and Substack (as HTML fallback) go through `cloud::smart_fetch_html`: try local fetch first; on bot-protection detection (Cloudflare challenge, DataDome, AWS WAF "Verifying your connection", etc.) escalate to `api.webclaw.io/v1/scrape`. Without `WEBCLAW_API_KEY` / `WEBCLAW_CLOUD_API_KEY` the extractor returns a typed `CloudError::NotConfigured` with an actionable signup link. With a key set, escalation is automatic. Every extractor stamps a `data_source: "local" | "cloud"` field on the response so callers can tell which path ran.
- **`cloud::synthesize_html` for cloud-bypassed extraction.** `api.webclaw.io/v1/scrape` deliberately does not return raw HTML; it returns a parsed bundle (`structured_data` JSON-LD blocks + `metadata` OG/meta tags + `markdown`). The new helper reassembles that bundle back into a minimal synthetic HTML doc (JSON-LD as `<script>` tags, metadata as OG `<meta>` tags, markdown in a `<pre>`) so existing local parsers run unchanged across both paths. No per-extractor code path branches are needed for "came from cloud" vs "came from local".
- **Trustpilot 2025 schema parser.** Trustpilot replaced their single-Organization + aggregateRating shape with three separate JSON-LD blocks: a site-level Organization (Trustpilot itself), a Dataset with a csvw:Table `mainEntity` carrying the per-star distribution for the target business, and an aiSummary + aiSummaryReviews block with the AI-generated summary and recent reviews. The parser walks all three, skips the site-level Org, picks the Dataset by `about.@id` matching the target domain, parses each csvw:column for rating buckets, computes weighted-average rating + total from the distribution, extracts the aiSummary text, and returns recent reviews with author / country / date / rating / title / text / likes.
- **OG-tag fallback in `ecommerce_product` for sites with no JSON-LD and sites with JSON-LD but empty offers.** Three paths now: `jsonld` (Schema.org Product with offers), `jsonld+og` (Product JSON-LD plus OG product tags filling in missing price), and `og_fallback` (no JSON-LD at all, build minimal payload from `og:title`, `og:image`, `og:description`, `product:price:amount`, `product:price:currency`, `product:availability`, `product:brand`). `has_og_product_signal()` gates the fallback on `og:type=product` or a price tag so blog posts don't get mis-classified as products.
- **URL-slug title fallback in `etsy_listing` for delisted / blocked pages.** When Etsy serves a placeholder page (`"etsy.com"`, `"Etsy - Your place to buy..."`, `"This item is unavailable"`), humanise the URL slug (`/listing/123/personalized-stainless-steel-tumbler` becomes `"Personalized Stainless Steel Tumbler"`) so callers always get a meaningful title. Plus shop falls through `offers[].seller.name` then top-level `brand` because Etsy uses both schemas depending on listing age.
- **Force-cloud-escalation in `amazon_product` when local HTML lacks Product JSON-LD.** Amazon A/B-tests JSON-LD presence. When local fetch succeeds but has no `Product` block and a cloud client is configured, the extractor force-escalates to the cloud which reliably surfaces title + description via its render engine. Added OG meta-tag fallback so the cloud's synthesized HTML output (OG tags only, no Amazon DOM IDs) still yields title / image / description.
- **AWS WAF "Verifying your connection" detector in `cloud::is_bot_protected`.** Trustpilot serves a `~565` byte interstitial with an `interstitial-spinner` CSS class. The detector now fires on that pattern with a `< 10_000` byte size gate to avoid false positives on real articles that happen to mention the phrase.

### Changed
- **`webclaw-fetch::FetchClient` gained an optional `cloud` field** via `with_cloud(CloudClient)`. Extractors reach it through `client.cloud()` to decide whether to escalate. `webclaw-server::AppState` reads `WEBCLAW_CLOUD_API_KEY` (preferred) or falls back to `WEBCLAW_API_KEY` only when inbound auth is not configured (open mode).
- **Consolidated `CloudClient` into `webclaw-fetch`.** Previously duplicated between `webclaw-mcp/src/cloud.rs` (302 LOC) and `webclaw-cli/src/cloud.rs` (80 LOC). Single canonical home with typed `CloudError` (`NotConfigured`, `Unauthorized`, `InsufficientPlan`, `RateLimited`, `ServerError`, `Network`, `ParseFailed`) that Display with actionable URLs; `From<CloudError> for String` bridge keeps pre-existing CLI / MCP call sites compiling unchanged during migration.

### Tests
- 215 unit tests passing in `webclaw-fetch` (100+ new, covering every extractor's matcher, URL parser, JSON-LD / OG fallback paths, and the cloud synthesis helper). `cargo clippy --workspace --release --no-deps` clean.

---

## [0.4.0] — 2026-04-22

### Added
- **`webclaw bench <url>` — per-URL extraction micro-benchmark (#26).** New subcommand. Fetches a URL once, runs the same extraction pipeline as `--format llm`, and prints a small ASCII table comparing raw-HTML tokens vs. llm-output tokens, bytes, and extraction time. Pass `--json` for a single-line JSON object (stable shape, easy to append to ndjson in CI). Pass `--facts <path>` with a file in the same schema as `benchmarks/facts.json` to get a fidelity column ("4/5 facts preserved"); URLs absent from the facts file produce no fidelity row, so uncurated sites aren't shown as 0/0. v1 uses an approximate tokenizer (`chars/4` for Latin text, `chars/2` when CJK dominates) — off by ±10% vs. a real BPE tokenizer, but the signal ("the LLM pipeline dropped 93% of the raw bytes") is the point. Output clearly labels counts as `≈ tokens` so nobody confuses them with a real tiktoken run. Swapping in `tiktoken-rs` later is a one-function change in `bench.rs`. Adding this as a `clap` subcommand rather than a flag also lays the groundwork for future subcommands without breaking the existing flag-based flow — `webclaw <url> --format llm` still works exactly as before.

- **`webclaw-server` — new OSS binary for self-hosting a REST API (#29).** Until now, `docs/self-hosting` promised a `webclaw-server` binary that only existed in the hosted-platform repo (closed source). The Docker image shipped two binaries while the docs advertised three, which sent self-hosters into a bug loop. This release closes the gap: a new crate at `crates/webclaw-server/` builds a minimal, stateless axum server that exposes the OSS extraction pipeline over HTTP with the same JSON shapes as api.webclaw.io. Endpoints: `GET /health`, `POST /v1/{scrape,crawl,map,batch,extract,summarize,diff,brand}`. Run with `webclaw-server --port 3000 [--host 0.0.0.0] [--api-key <bearer>]` or the matching `WEBCLAW_PORT` / `WEBCLAW_HOST` / `WEBCLAW_API_KEY` env vars. Bearer auth is constant-time (via `subtle::ConstantTimeEq`); open mode (no key) is allowed on `127.0.0.1` for local development.

  What self-hosting gives you: the full extraction pipeline, Crawler, sitemap discovery, brand/diff, LLM extract/summarize (via Ollama or your own OpenAI/Anthropic key). What it does *not* give you: anti-bot bypass (Cloudflare, DataDome, WAFs), headless JS rendering, async job queues, multi-tenant auth/billing, domain-hints and proxy routing — those require the hosted backend at api.webclaw.io and are intentionally not open-source. The self-hosting docs have been updated to reflect this split honestly.

- **`crawl` endpoint runs synchronously and hard-caps at 500 pages / 20 concurrency.** No job queue, no background workers — a naive caller can't OOM the process. `batch` caps at 100 URLs / 20 concurrency for the same reason. For unbounded crawls use the hosted API.

### Changed
- **Docker image now ships three binaries**, not two. `Dockerfile` and `Dockerfile.ci` both add `webclaw-server` to `/usr/local/bin/` and `EXPOSE 3000` for documentation. The entrypoint shim is unchanged: `docker run IMAGE webclaw-server --port 3000` Just Works, and the CLI/URL pass-through from v0.3.19 is preserved.

### Docs
- Rewrote `docs/self-hosting` on the landing site to differentiate OSS (self-hosted REST) from the hosted platform. Added a capability matrix so new users don't have to read the repo to figure out why Cloudflare-protected sites still 403 when pointing at their own box.

### Fixed
- **Dead-code warning on `cargo install webclaw-mcp` (#30).** `rmcp` 1.3.x changed how the `#[tool_handler]` macro reads the `tool_router` struct field — it now goes through a derived trait impl instead of referencing the field by name, so rustc's dead-code lint no longer sees it. The field is still essential (dropping it unregisters every MCP tool), just invisible to the lint. Annotated with `#[allow(dead_code)]` and a comment explaining why. No behaviour change. Warning disappears on the next `cargo install`.

---

## [0.3.19] — 2026-04-17

### Fixed
- **Docker image can be used as a FROM base again.** v0.3.13 switched the Docker `CMD` to `ENTRYPOINT ["webclaw"]` so that `docker run IMAGE https://example.com` would pass the URL through as expected. That change trapped a different use case: downstream Dockerfiles that `FROM ghcr.io/0xmassi/webclaw` and set their own `CMD ["./setup.sh"]` — the child's `./setup.sh` became the first arg to `webclaw`, which tried to fetch it as a URL and failed with `error sending request for uri (https://./setup.sh)`. Both `Dockerfile` and `Dockerfile.ci` now use a small `docker-entrypoint.sh` shim that forwards flags (`-*`) and URLs (`http://`, `https://`) to `webclaw`, but `exec`s anything else directly. All four use cases now work: `docker run IMAGE https://example.com`, `docker run IMAGE --help`, child-image `CMD ["./setup.sh"]`, and `docker run IMAGE bash` for debugging. Default `CMD` is `["webclaw", "--help"]`.

---

## [0.3.18] — 2026-04-16

### Fixed
- **UTF-8 char boundary panic in `webclaw-core::extractor::find_content_position` (#16).** After rejecting a match that fell inside image syntax (`![...](...)`), the scan advanced `search_from` by a single byte. If the rejected match started on a multi-byte character (Cyrillic, CJK, accented Latin, emoji), the next `markdown[search_from..]` slice landed mid-char and panicked with `byte index N is not a char boundary; it is inside 'X'`. Repro was `webclaw https://bruler.ru/about_brand -f json`. Now advances by `needle.len()` — always a valid char boundary, and faster because it skips the whole rejected match instead of re-scanning inside it. Two regression tests cover multi-byte rejected matches and all-rejected cycles in Cyrillic text.

---

## [0.3.17] — 2026-04-16

### Changed
- **`webclaw-fetch::sitemap::parse_robots_txt` now does proper directive parsing.** The previous `trimmed[..8].eq_ignore_ascii_case("sitemap:")` slice couldn't handle "Sitemap :" (space before colon) from bad generators, didn't strip inline `# ...` comments, and would have returned empty/garbage values if a directive line had no URL. Now splits on the first colon, matches any-case `sitemap` as the directive name, strips comments, and requires the value to contain `://` before accepting it. Eight new unit tests cover case variants, space-before-colon, inline comments, non-URL values, and non-sitemap directives.
- **`webclaw-fetch::crawler::is_cancelled` uses `Ordering::Acquire`** (was `Relaxed`). Technically equivalent on x86/arm64 for single-word loads, but the explicit ordering documents the synchronization intent for readers and the compiler.

### Added
- **`webclaw-mcp` caches the Firefox FetchClient lazily.** Tool calls that repeatedly request the Firefox profile without cookies used to build a fresh reqwest pool + TLS stack per call; a single `OnceLock` keeps the client alive for the life of the server. Chrome (default) and Random (by design per-call) are unaffected.

---

## [0.3.16] — 2026-04-16

### Hardened
- **Response body caps across fetch + LLM providers (P2).** Every HTTP response buffered from the network is now rejected if it exceeds a hard size cap. `webclaw-fetch::Response::from_wreq` caps HTML/doc responses at 50 MB (before the allocation pays for anything and as a belt-and-braces check after `bytes().await`); `webclaw-llm` providers (anthropic / openai / ollama) cap JSON responses at 5 MB via a shared `response_json_capped` helper. Previously an adversarial or runaway upstream could push unbounded memory into the process. Closes the DoS-via-giant-body class of bugs noted in the audit.
- **Crawler frontier cap (P2).** After each depth level the frontier is truncated to `max(max_pages × 10, 100)` entries, keeping the most recently discovered links. Dense pages (tag clouds, search results) used to push the frontier into the tens of thousands even after `max_pages` halted new fetches, keeping string allocations alive long after the crawl was effectively done.
- **Glob pattern validation (P2).** User-supplied `include_patterns` / `exclude_patterns` passed to the crawler are now rejected if they contain more than 4 `**` wildcards or exceed 1024 chars. The backtracking matcher degrades exponentially on deeply-nested `**` against long paths; this keeps adversarial config files from weaponising it.

### Cleanup
- **Removed blanket `#![allow(dead_code)]` in `webclaw-cli/src/main.rs`.** No dead code surfaced; the suppression was obsolete.
- **`.gitignore`: replaced overbroad `*.json` with specific local-artifact patterns.** The previous rule would have swallowed `package.json` / `components.json` / `.smithery/*.json` if they were ever modified.

---

## [0.3.15] — 2026-04-16

### Fixed
- **Batch/crawl no longer panics on semaphore close (P1).** Three `permit.acquire().await.expect("semaphore closed")` call sites in `webclaw-fetch` (`client::fetch_batch`, `client::fetch_and_extract_batch_with_options`, `crawler` inner loop) now surface a typed `FetchError::Build("semaphore closed before acquire")` or a failed `PageResult` instead of panicking the spawned task. Under normal operation nothing changes; under shutdown-race or adversarial runtime state, the caller sees one failed entry in the batch instead of losing the task silently to the runtime's panic handler. Surfaced by the 2026-04-16 workspace audit.

---

## [0.3.14] — 2026-04-16

### Security
- **`--on-change` command injection closed (P0).** The `--on-change` flag on `webclaw watch` and its multi-URL variant used to pipe the whole user-supplied string through `sh -c`. Anyone (or any LLM driving the MCP surface, or any config file parsed on the user's behalf) that could influence the flag value could execute arbitrary shell. The command is now tokenized with `shlex` and executed directly via `Command::new(prog).args(args)`, so metacharacters like `;`, `&&`, `|`, `$()`, `<(...)`, and env expansion no longer fire. A `WEBCLAW_ALLOW_SHELL=1` escape hatch is available for users who genuinely need pipelines; it logs a warning on every invocation so it can't slip in silently. Surfaced by the 2026-04-16 workspace audit.

---

## [0.3.13] — 2026-04-10

### Fixed
- **Docker CMD replaced with ENTRYPOINT**: both `Dockerfile` and `Dockerfile.ci` now use `ENTRYPOINT ["webclaw"]` instead of `CMD ["webclaw"]`. CLI arguments (e.g. `docker run webclaw https://example.com`) now pass through correctly instead of being ignored.

---

## [0.3.12] — 2026-04-10

### Added
- **Crawl scope control**: new `allow_subdomains` and `allow_external_links` fields on `CrawlConfig`. By default crawls stay same-origin. Enable `allow_subdomains` to follow sibling/child subdomains (e.g. blog.example.com from example.com), or `allow_external_links` for full cross-origin crawling. Root domain extraction uses a heuristic that handles two-part TLDs (co.uk, com.au).

---

## [0.3.11] — 2026-04-10

### Added
- **Sitemap fallback paths**: discovery now tries `/sitemap_index.xml`, `/wp-sitemap.xml`, and `/sitemap/sitemap-index.xml` in addition to the standard `/sitemap.xml`. Sites using WordPress or non-standard sitemap locations are now discovered without needing external search.

---

## [0.3.10] — 2026-04-10

### Changed
- **Fetch timeout reduced from 30s to 12s**: prevents cascading slowdowns when proxies are unresponsive. Worst-case per-URL drops from ~94s to ~25s.
- **Retry attempts reduced from 3 to 2**: combined with shorter timeout, total worst-case is 12s + 1s delay + 12s = 25s instead of 30s + 1s + 30s + 3s + 30s = 94s.

---

## [0.3.9] — 2026-04-04

### Fixed
- **Layout tables rendered as sections**: tables used for page layout (containing block elements like `<p>`, `<div>`, `<hr>`) are now rendered as standalone sections instead of pipe-delimited markdown tables. Fixes Drudge Report and similar sites where all content was flattened into a single unreadable line. (by [@devnen](https://github.com/devnen) in #14)
- **Stack overflow on deeply nested HTML**: pages with 200+ DOM nesting levels (e.g., Express.co.uk live blogs) no longer overflow the stack. Two-layer fix: depth guard in markdown.rs falls back to iterator-based text collection at depth 200, and `extract_with_options()` spawns an 8 MB worker thread for safety on Windows. (by [@devnen](https://github.com/devnen) in #14)
- **Noise filter swallowing content in malformed HTML**: `<form>` tags no longer unconditionally treated as noise — ASP.NET page-wrapping forms (>500 chars) are preserved. Safety valve prevents unclosed noise containers (header/footer with >5000 chars) from absorbing entire page content. (by [@devnen](https://github.com/devnen) in #14)

### Changed
- **Bold/italic block passthrough**: `<b>`/`<strong>`/`<em>`/`<i>` tags containing block-level children (e.g., Drudge wrapping columns in `<b>`) now act as transparent containers instead of collapsing everything into inline bold/italic. (by [@devnen](https://github.com/devnen) in #14)

---

## [0.3.8] — 2026-04-03

### Fixed
- **MCP research token overflow**: research results are now saved to `~/.webclaw/research/` and the MCP tool returns file paths + findings instead of the full report. Prevents "exceeds maximum allowed tokens" errors in Claude/Cursor.
- **Research caching**: same query returns cached result instantly without spending credits.
- **Anthropic rate limit throttling**: 60s delay between LLM calls in research to stay under Tier 1 limits (50K input tokens/min).

### Added
- **`dirs` dependency** for `~/.webclaw/research/` path resolution.

---
## [0.3.7] — 2026-04-03

### Added
- **`--research` CLI flag**: run deep research via the cloud API. Prints report to stdout and saves full result (report + sources + findings) to a JSON file. Supports `--deep` for longer reports.
- **MCP extract/summarize cloud fallback**: when no local LLM is available, these tools now fall back to the cloud API instead of erroring. Set `WEBCLAW_API_KEY` for automatic fallback.
- **MCP research structured output**: the research tool now returns structured JSON (report + sources + findings + metadata) instead of raw text, so agents can reference individual findings and source URLs.

---

## [0.3.6] — 2026-04-02

### Added
- **Structured data in markdown/LLM output**: `__NEXT_DATA__`, SvelteKit, and JSON-LD data now appears as a `## Structured Data` section with a JSON code block at the end of `-f markdown` and `-f llm` output. Works with `--only-main-content` and all other flags.

### Fixed
- **Homebrew CI**: formula now updates all 4 platform checksums after Docker build completes, preventing SHA mismatch on Linux installs (#12).

---

## [0.3.5] — 2026-04-02

### Added
- **`__NEXT_DATA__` extraction**: Next.js pages now have their `pageProps` JSON extracted into `structured_data`. Contains prices, product info, page state, and other data that isn't in the visible HTML. Tested on 45 sites — 13 now return rich structured data (BBC, Forbes, Nike, Stripe, TripAdvisor, Glassdoor, NASA, etc.).

---

## [0.3.4] — 2026-04-01

### Added
- **SvelteKit data island extraction**: extracts structured JSON from `kit.start()` data arrays. Handles unquoted JS object keys by converting to valid JSON before parsing. Data appears in the `structured_data` field.

### Changed
- **License changed from MIT to AGPL-3.0**.

---

## [0.3.3] — 2026-04-01

### Changed
- **Replaced custom TLS stack with wreq**: migrated from webclaw-tls (patched rustls/h2/hyper/reqwest) to [wreq](https://github.com/0x676e67/wreq) by [@0x676e67](https://github.com/0x676e67). wreq uses BoringSSL for TLS and the [http2](https://github.com/0x676e67/http2) crate for HTTP/2 fingerprinting — both battle-tested with 60+ browser profiles.
- **Removed all `[patch.crates-io]` entries**: consumers no longer need to patch rustls, h2, hyper, hyper-util, or reqwest. Just depend on webclaw normally.
- **Browser profiles rebuilt on wreq's Emulation API**: Chrome 145, Firefox 135, Safari 18, Edge 145 with correct TLS options (cipher suites, curves, GREASE, ECH, PSK session resumption), HTTP/2 SETTINGS ordering, pseudo-header order, and header wire order.
- **Better TLS compatibility**: BoringSSL handles more server configurations than patched rustls (e.g. servers that previously returned IllegalParameter alerts).

### Removed
- webclaw-tls dependency and all 5 forked crates (webclaw-rustls, webclaw-h2, webclaw-hyper, webclaw-hyper-util, webclaw-reqwest).

### Acknowledgments
- TLS and HTTP/2 fingerprinting powered by [wreq](https://github.com/0x676e67/wreq) and [http2](https://github.com/0x676e67/http2) by [@0x676e67](https://github.com/0x676e67), who pioneered browser-grade HTTP/2 fingerprinting in Rust.

---

## [0.3.2] — 2026-03-31

### Added
- **`--cookie-file` flag**: load cookies from JSON files exported by browser extensions (EditThisCookie, Cookie-Editor). Format: `[{name, value, domain, ...}]`.
- **MCP `cookies` parameter**: the `scrape` tool now accepts a `cookies` array for authenticated scraping.
- **Combined cookies**: `--cookie` and `--cookie-file` can be used together and merge automatically.

---

## [0.3.1] — 2026-03-30

### Added
- **Cookie warmup fallback**: when a fetch returns an Akamai challenge page, automatically visits the homepage first to collect `_abck`/`bm_sz` cookies, then retries the original URL. Enables extraction of Akamai-protected subpages (e.g. fansale ticket pages) without JS rendering.

### Changed
- Fixed HTTP header wire order (accept/user-agent were in wrong positions) and added H2 PRIORITY flag in HEADERS frames.
- `FetchResult.headers` now uses `http::HeaderMap` instead of `HashMap<String, String>` — avoids per-response allocation, preserves multi-value headers.

## [0.3.0] — 2026-03-29

### Changed
- **Replaced primp with webclaw-tls**: switched to custom TLS fingerprinting stack.
- **Browser profiles**: Chrome 146 (Win/Mac), Firefox 135+, Safari 18, Edge 146 — captured from real browsers.
- **HTTP/2 fingerprinting**: SETTINGS frame ordering and pseudo-header ordering based on concepts pioneered by [@0x676e67](https://github.com/0x676e67).

### Fixed
- **HTTPS completely broken (#5)**: primp's forked rustls rejected valid certificates (UnknownIssuer on cross-signed chains like example.com). Fixed by using native OS root CAs alongside Mozilla bundle.
- **Unknown certificate extensions**: servers returning SCT in certificate entries no longer cause TLS errors.

### Added
- **Native root CA support**: uses OS trust store (macOS Keychain, Windows cert store) in addition to webpki-roots.
- **HTTP/2 fingerprinting**: SETTINGS frame ordering and pseudo-header ordering match real browsers.
- **Per-browser header ordering**: HTTP headers sent in browser-specific wire order.
- **Bandwidth tracking**: atomic byte counters shared across cloned clients.

---

## [0.2.2] — 2026-03-27

### Fixed
- **`cargo install` broken with primp 1.2.0**: added missing `reqwest` patch to `[patch.crates-io]`. primp moved to reqwest 0.13 which requires a patched fork.
- **Weekly dependency check**: CI now runs every Monday to catch primp patch drift before users hit it.

---

## [0.2.1] — 2026-03-27

### Added
- **Docker image on GHCR**: `docker run ghcr.io/0xmassi/webclaw` — auto-built on every release
- **QuickJS data island extraction**: inline `<script>` execution catches `window.__PRELOADED_STATE__`, Next.js hydration data, and other JS-embedded content

### Fixed
- Docker CI now runs as part of the release workflow (was missing, image was never published)

---

## [0.2.0] — 2026-03-26

### Added
- **DOCX extraction**: auto-detected by Content-Type or URL extension, outputs markdown with headings
- **XLSX/XLS extraction**: spreadsheets converted to markdown tables, multi-sheet support via calamine
- **CSV extraction**: parsed with quoted field handling, output as markdown table
- **HTML output format**: `-f html` returns sanitized HTML from the extracted content
- **Multi-URL watch**: `--watch` now works with `--urls-file` to monitor multiple URLs in parallel
- **Batch + LLM extraction**: `--extract-prompt` and `--extract-json` now work with multiple URLs
- **Scheduled batch watch**: watch multiple URLs with aggregate change reports and per-URL diffs

---

## [0.1.7] — 2026-03-26

### Fixed
- `--only-main-content`, `--include`, and `--exclude` now work in batch mode (#3)

---

## [0.1.6] — 2026-03-26

### Added
- `--watch`: monitor a URL for changes at a configurable interval with diff output
- `--watch-interval`: seconds between checks (default: 300)
- `--on-change`: run a command when changes are detected (diff JSON piped to stdin)
- `--webhook`: POST JSON notifications on crawl/batch complete and watch changes. Auto-formats for Discord and Slack webhooks

---

## [0.1.5] — 2026-03-26

### Added
- `--output-dir`: save each page to a separate file instead of stdout. Works with single URL, crawl, and batch modes
- CSV input with custom filenames: `url,filename` format in `--urls-file`
- Root URLs use `hostname/index.ext` to avoid collisions in batch mode
- Subdirectories created automatically from URL path structure

---

## [0.1.4] — 2026-03-26

### Added
- QuickJS integration for extracting data from inline JavaScript (NYTimes +168%, Wired +580% more content)
- Executes inline `<script>` tags in a sandboxed runtime to capture `window.__*` data blobs
- Parses Next.js RSC flight data (`self.__next_f`) for App Router sites
- Smart text filtering rejects CSS, base64, file paths, and code — only keeps readable prose
- Feature-gated with `quickjs` feature flag (enabled by default, disable for WASM builds)

---

## [0.1.3] — 2026-03-25

### Added
- Crawl streaming: real-time progress on stderr as pages complete (`[2/50] OK https://... (234ms, 1523 words)`)
- Crawl resume/cancel: `--crawl-state <path>` saves progress on Ctrl+C and resumes from where it left off
- MCP server proxy support via `WEBCLAW_PROXY` and `WEBCLAW_PROXY_FILE` env vars

### Changed
- Crawl results now expose visited set and remaining frontier for accurate state persistence

---

## [0.1.2] — 2026-03-25

### Changed
- Default TLS profile switched from Chrome145/Win to Safari26/Mac (highest pass rate across CF-protected sites)
- Plain client fallback: when impersonated TLS gets connection error or 403, automatically retries without impersonation (fixes ycombinator.com, producthunt.com, and similar sites)

### Fixed
- Reddit scraping: use plain HTTP client for `.json` endpoint (TLS fingerprinting was getting blocked)

### Added
- YouTube transcript extraction infrastructure in webclaw-core (caption track parsing, timed text XML parser) — wired up when cloud API launches

---

## [0.1.1] — 2026-03-24

### Fixed
- MCP server now identifies as `webclaw-mcp` instead of `rmcp` in the MCP handshake
- Research tool polling caps at 200 iterations (~10 min) instead of looping forever
- CLI returns non-zero exit codes on errors (invalid format, fetch failures, missing LLM)
- Text format output strips markdown table syntax (`| --- |` pipes)
- All MCP tools validate URLs before network calls with clear error messages
- Cloud API HTTP client has 60s timeout instead of no timeout
- Local fetch calls timeout after 30s to prevent hanging on slow servers
- Diff cloud fallback computes actual diff instead of returning raw scrape JSON
- FetchClient startup failure logs and exits gracefully instead of panicking

### Added
- Upper bounds: batch capped at 100 URLs, crawl capped at 500 pages

---

## [0.1.0] — 2026-03-18

First public release. Full-featured web content extraction toolkit for LLMs.

### Core Extraction
- Readability-style content scoring with text density, semantic tags, and link density penalties
- Exact CSS class token noise filtering with body-force fallback for SPAs
- HTML → markdown conversion with URL resolution, image alt text, srcset optimization
- 9-step LLM text optimization pipeline (67% token reduction vs raw HTML)
- JSON data island extraction (React, Next.js, Contentful CMS)
- YouTube transcript extraction (title, channel, views, duration, description)
- Lazy-loaded image detection (data-src, data-lazy-src, data-original)
- Brand identity extraction (name, colors, fonts, logos, OG image)
- Content change tracking / diff engine
- CSS selector filtering (include/exclude)

### Fetching & Crawling
- TLS fingerprint impersonation via Impit (Chrome 142, Firefox 144, random mode)
- BFS same-origin crawler with configurable depth, concurrency, and delay
- Sitemap.xml and robots.txt discovery
- Batch multi-URL concurrent extraction
- Per-request proxy rotation from pool file
- Reddit JSON API and LinkedIn post extractors

### LLM Integration
- Provider chain: Ollama (local-first) → OpenAI → Anthropic
- JSON schema extraction (structured data from pages)
- Natural language prompt extraction
- Page summarization with configurable sentence count

### PDF
- PDF text extraction via pdf-extract
- Auto-detection by Content-Type header

### MCP Server
- 8 tools: scrape, crawl, map, batch, extract, summarize, diff, brand
- stdio transport for Claude Desktop, Claude Code, and any MCP client
- Smart Fetch: local extraction first, cloud API fallback

### CLI
- 4 output formats: markdown, JSON, plain text, LLM-optimized
- CSS selector filtering, crawling, sitemap discovery
- Brand extraction, content diffing, LLM features
- Browser profile selection, proxy support, stdin/file input

### Infrastructure
- Docker multi-stage build with Ollama sidecar
- Deploy script for Hetzner VPS
