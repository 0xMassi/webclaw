/// CLI entry point -- wires webclaw-core and webclaw-fetch into a single command.
/// All extraction and fetching logic lives in sibling crates; this is pure plumbing.
mod bench;

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;
use webclaw_core::{
    ChangeStatus, ContentDiff, ExtractionOptions, ExtractionResult, Metadata, extract_with_options,
    to_citations_after, to_llm_text,
};
use webclaw_fetch::{
    BatchExtractResult, BrowserProfile, CrawlConfig, CrawlResult, Crawler, FetchClient,
    FetchConfig, FetchResult, PageResult, PageSink, SitemapEntry,
};
use webclaw_llm::LlmProvider;
use webclaw_pdf::PdfMode;

/// Known anti-bot challenge page titles (case-insensitive prefix match).
const ANTIBOT_TITLES: &[&str] = &[
    "just a moment",
    "attention required",
    "access denied",
    "checking your browser",
    "please wait",
    "one more step",
    "verify you are human",
    "bot verification",
    "security check",
    "ddos protection",
];

/// URL host/path fragments that indicate a GDPR/cookie consent redirect.
const CONSENT_URL_FRAGMENTS: &[&str] = &[
    "://consent.",
    "/consent?",
    "/consent/",
    "collectconsent",
    "consentcheck",
    "/cmp/",
    "guce.advertising.com",
];

/// English consent-wall title prefixes. Many providers localize this page, so
/// this is a best-effort secondary signal. URL shape is the primary signal.
const CONSENT_TITLES: &[&str] = &[
    "before you continue",
    "your privacy choices",
    "we value your privacy",
    "we care about your privacy",
    "cookie consent",
    "consent required",
];

/// Detect why a page returned empty or near-empty content.
#[derive(Debug, PartialEq, Eq)]
enum EmptyReason {
    /// Anti-bot challenge page (Cloudflare, Akamai, etc.)
    Antibot,
    /// GDPR/cookie consent redirect.
    ConsentWall,
    /// JS-only SPA that returns an empty shell without a browser
    JsRequired,
    /// Page has content.
    None,
}

fn detect_empty(result: &ExtractionResult) -> EmptyReason {
    // Consent walls can have a tiny body, so check before the content
    // short-circuit.
    if is_consent_wall(result) {
        return EmptyReason::ConsentWall;
    }

    // Has real content. Nothing to warn about.
    if result.metadata.word_count > 50 || !result.content.markdown.is_empty() {
        return EmptyReason::None;
    }

    // Check for known anti-bot challenge titles
    if let Some(ref title) = result.metadata.title {
        let lower = title.to_lowercase();
        if ANTIBOT_TITLES.iter().any(|t| lower.starts_with(t)) {
            return EmptyReason::Antibot;
        }
    }

    // Empty content with no title or a generic SPA shell = JS-only site
    if result.metadata.word_count == 0 && result.content.links.is_empty() {
        return EmptyReason::JsRequired;
    }

    EmptyReason::None
}

/// A consent wall is identified by either:
/// 1. The final URL pointing at a known consent host/path, or
/// 2. A consent-wall title prefix with a very small body.
fn is_consent_wall(result: &ExtractionResult) -> bool {
    if let Some(ref url) = result.metadata.url {
        let lower = url.to_ascii_lowercase();
        if CONSENT_URL_FRAGMENTS
            .iter()
            .any(|fragment| lower.contains(fragment))
        {
            return true;
        }
    }

    if result.metadata.word_count <= 50
        && let Some(ref title) = result.metadata.title
    {
        let lower = title.to_lowercase();
        if CONSENT_TITLES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            return true;
        }
    }

    false
}

fn warn_empty(url: &str, reason: &EmptyReason) {
    match reason {
        EmptyReason::Antibot => eprintln!(
            "\x1b[33mwarning:\x1b[0m Anti-bot protection detected on {url}\n\
             This site requires CAPTCHA solving or browser rendering.\n\
             For a Cloudflare challenge, try --solve (needs a `browser` build).\n\
             Use the webclaw Cloud API for automatic bypass: https://webclaw.io/pricing"
        ),
        EmptyReason::ConsentWall => eprintln!(
            "\x1b[33mwarning:\x1b[0m GDPR/cookie consent wall detected on {url}\n\
             The site redirected to a consent page and returned no usable content.\n\
             Try a different region via --proxy, or pass a pre-accepted consent cookie\n\
             via --cookie / --cookie-file."
        ),
        EmptyReason::JsRequired => eprintln!(
            "\x1b[33mwarning:\x1b[0m No content extracted from {url}\n\
             This site requires JavaScript rendering (SPA).\n\
             Use the webclaw Cloud API for JS rendering: https://webclaw.io/pricing"
        ),
        EmptyReason::None => {}
    }
}

#[derive(Parser)]
#[command(name = "webclaw", about = "Extract web content for LLMs", version = VERSION)]
struct Cli {
    /// Optional subcommand. When omitted, the CLI falls back to the
    /// traditional flag-based flow (URL + --format, --crawl, etc.).
    /// Subcommands are used for flows that don't fit that model.
    #[command(subcommand)]
    command: Option<Commands>,

    /// URLs to fetch (multiple allowed)
    #[arg()]
    urls: Vec<String>,

    /// File with URLs (one per line)
    #[arg(long)]
    urls_file: Option<String>,

    /// Assert that the URL has been handled for shell escaping. Suppresses
    /// the URL-truncation stderr warning. Use when the URL is intentionally
    /// passed with an empty/keyless query (e.g. legacy CGI) or when a
    /// trailing `&` is genuinely part of the URL. The URL is fetched as-is
    /// (no extra normalization beyond the standard scheme prepend).
    #[arg(long)]
    url_encoded: bool,

    /// Output format (markdown, json, text, llm, html)
    #[arg(short, long, default_value = "markdown")]
    format: OutputFormat,

    /// Browser to impersonate
    #[arg(short, long, default_value = "chrome")]
    browser: Browser,

    /// Proxy URL (http://user:pass@host:port or https://host:port)
    #[arg(short, long, env = "WEBCLAW_PROXY", hide_env_values = true)]
    proxy: Option<String>,

    /// File with proxies (host:port:user:pass, one per line). Rotates per request.
    #[arg(long, env = "WEBCLAW_PROXY_FILE", hide_env_values = true)]
    proxy_file: Option<String>,

    /// Request timeout in seconds
    #[arg(short, long, default_value = "30")]
    timeout: u64,

    /// Keep fetched pages on disk and reuse them on later runs instead of
    /// fetching again, in WEBCLAW_CACHE_DIR or ~/.cache/webclaw. Entries never
    /// expire; delete the directory to refresh
    #[arg(long)]
    cache: bool,

    /// Render pages in a local Chrome. auto renders only pages whose content needs JavaScript; needs a build with the `browser` feature
    #[arg(long, value_enum, default_value_t = RenderMode::Auto)]
    render: RenderMode,

    /// Render with an already running browser at this DevTools endpoint (ws://... or http://host:port) instead of launching Chrome
    #[arg(long)]
    cdp_url: Option<String>,

    /// Keep the local browser's cookies, local storage and IndexedDB in this
    /// directory, created when missing, so a login carries over to later
    /// runs. Web pages then load in the browser with the profile's cookies,
    /// and --cookie and --cookie-file cookies are saved in the profile. One
    /// run at a time per directory; use a directory of its own, not your
    /// everyday Chrome profile
    #[arg(long, value_name = "DIR", conflicts_with = "cdp_url")]
    browser_profile: Option<PathBuf>,

    /// When rendering, also wait until an element matching this CSS selector exists
    #[arg(long)]
    wait_for: Option<String>,

    /// Get past Cloudflare challenge pages in a local Chrome: render a page
    /// that comes back as one, wait for it to clear or click its checkbox,
    /// and read the page it leads to. Later pages of that site are fetched
    /// with the clearance cookie it got. A Turnstile checkbox in any page
    /// that gets rendered is clicked too. Needs a build with the `browser`
    /// feature
    #[arg(long, conflicts_with_all = ["screenshot", "save_pdf", "save_mhtml"])]
    solve: bool,

    /// When rendering a page for its content, skip these resources: a comma
    /// list of images, fonts and media, or none. Screenshots and PDFs
    /// load everything
    #[arg(long, value_name = "TYPES", conflicts_with_all = ["screenshot", "save_pdf", "save_mhtml"])]
    block: Option<webclaw_fetch::Block>,

    /// Steps to run in the local browser once the page has loaded, before it
    /// is read, in the hosted API's format: a JSON array, or @FILE to read
    /// one, e.g. '[{"type": "click", "selector": "button.more", "repeat":
    /// 10}]'. Steps: click, type, press, scroll, wait, waitForSelector and
    /// executeJavascript; see the README. The values of executeJavascript
    /// steps come back as js_results: in the output with -f json, on stderr
    /// otherwise. Renders the page even with --render auto
    #[arg(long, value_name = "JSON", conflicts_with_all = ["crawl", "watch", "map", "brand", "cloud"])]
    actions: Option<String>,

    /// Scroll to the end of the page in the local browser, again and again
    /// until it stops growing, at most TIMES times (default 20, or
    /// --scroll=50), before reading it. Runs before --actions
    #[arg(
        long,
        value_name = "TIMES",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "20",
        conflicts_with_all = ["crawl", "watch", "map", "brand", "cloud"]
    )]
    scroll: Option<u32>,

    /// Extract from local HTML file instead of fetching
    #[arg(long)]
    file: Option<String>,

    /// Read HTML from stdin
    #[arg(long)]
    stdin: bool,

    /// Include metadata in output (always included in JSON)
    #[arg(long)]
    metadata: bool,

    /// Output raw fetched HTML instead of extracting
    #[arg(long)]
    raw_html: bool,

    /// Keep only the parts of the page that answer this question, under their
    /// headings. With --crawl, also follow the links most likely to answer it
    /// first and list pages best match first. With --map, list URLs best match
    /// first by the words in their paths
    #[arg(long, value_name = "QUESTION", conflicts_with = "watch")]
    query: Option<String>,

    /// Print the page as chunks for retrieval, one JSON object per line (one
    /// JSON array with -f json) with url, index, heading_path, tokens and
    /// markdown text. Paragraphs, lists, tables and code blocks stay whole;
    /// each chunk stays under about this many tokens. Works with several URLs
    /// and --crawl (chunks per page)
    #[arg(long, value_name = "TOKENS", value_parser = clap::value_parser!(u32).range(1..),
          conflicts_with_all = ["select", "xpath", "find_text", "find_regex", "tables", "screenshot", "save_pdf", "save_mhtml", "output_dir", "metadata", "watch", "diff_with", "brand", "map", "raw_html", "extract_json", "extract_prompt", "summarize", "research"])]
    chunk: Option<u32>,

    /// With --chunk, start each chunk with up to this many tokens from the end
    /// of the one before (at most half the chunk size)
    #[arg(long, value_name = "TOKENS", requires = "chunk", default_value_t = 0)]
    chunk_overlap: u32,

    /// Move link and image URLs out of the markdown into a numbered
    /// Links list at the end: `[text](url)` becomes `text[^1]`, and a URL
    /// used twice keeps one number. Applies to -f markdown and to the
    /// markdown inside -f json and jsonl, for one URL, several or --crawl
    #[arg(long, conflicts_with_all = ["chunk", "raw_html", "capture_network", "select", "xpath", "find_text", "find_regex", "tables", "screenshot", "save_pdf", "watch", "diff_with", "brand", "map", "extract_json", "extract_prompt", "summarize", "research"])]
    citations: bool,

    /// Print what a CSS selector picks as a JSON array. End it in `::text`
    /// (the default), `::attr(NAME)` or `::html`, e.g. "h3 a::attr(href)"
    #[arg(long, value_name = "SELECTOR", group = "pick")]
    select: Option<String>,

    /// Print what an XPath 1.0 expression picks as a JSON array: element and
    /// text values as text, attributes as values, e.g. "//h3/a/@title"
    #[arg(long, value_name = "EXPRESSION", group = "pick")]
    xpath: Option<String>,

    /// Print the page's data tables as JSON: caption, headers and rows of
    /// cell text. Layout tables are skipped
    #[arg(
        long,
        group = "pick",
        conflicts_with_all = ["similar", "fields", "exact", "case_sensitive", "crawl", "watch", "map"]
    )]
    tables: bool,

    /// Print the deepest elements whose text contains TEXT, any case, as a
    /// JSON array of {"selector", "text"}, each with a CSS path to the element
    #[arg(long, value_name = "TEXT", groups = ["pick", "find"])]
    find_text: Option<String>,

    /// Like --find-text with a regular expression, e.g. "^£\d+\.\d\d$"
    #[arg(long, value_name = "REGEX", groups = ["pick", "find"])]
    find_regex: Option<String>,

    /// With --find-text, match the whole text instead of a part of it
    #[arg(long, requires = "find_text", conflicts_with_all = ["select", "xpath", "find_regex"])]
    exact: bool,

    /// With --find-text or --find-regex, match letter case
    #[arg(long, requires = "find")]
    case_sensitive: bool,

    /// Use the first element --select, --xpath, --find-text or --find-regex
    /// picks as an example and print every element like it, with one CSS
    /// selector that picks them all: {"selector", "matches"}
    #[arg(long, requires = "pick")]
    similar: bool,

    /// With --select, --xpath or --similar, print one JSON object per match
    /// instead: NAME=SELECTOR run inside each match (repeatable), e.g. `--field
    /// price=.price_color`, or with --xpath `--field "price=.//p[@class='price_color']"`.
    /// With --similar the fields are CSS and the output is {"selector", "records"}
    #[arg(long = "field", value_name = "NAME=SELECTOR", requires = "pick")]
    fields: Vec<String>,

    /// Save a 1280x720 screenshot of the rendered page to this file, JPEG
    /// when it ends in .jpg or .jpeg, PNG otherwise. Needs the `browser` feature
    #[arg(long, value_name = "FILE", conflicts_with_all = ["pick", "crawl", "watch", "map", "format", "summarize", "cloud", "output_dir", "diff_with", "brand", "raw_html", "pdf_mode", "metadata", "query", "extract_json", "extract_prompt", "research", "include", "exclude", "only_main_content", "stdin", "file"])]
    screenshot: Option<PathBuf>,

    /// With --screenshot, capture the whole scrollable page
    #[arg(long, requires = "screenshot")]
    full_page: bool,

    /// Render the page in the local browser and print the responses it
    /// loaded as a JSON array instead of the page: url, method, status,
    /// resource type, MIME type, request headers and the body of text
    /// responses. PATTERN is a regular expression the URL must match, given
    /// with an equals sign: --capture-network=/api/. With --actions or
    /// --scroll, the steps run first, so the responses they trigger are
    /// captured too. Needs the `browser` feature
    #[arg(
        long,
        value_name = "PATTERN",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "",
        conflicts_with_all = [
            "pick", "crawl", "watch", "map", "screenshot", "save_pdf", "save_mhtml", "brand", "raw_html", "cloud",
            "format", "query", "chunk", "include", "exclude", "only_main_content", "metadata"
        ]
    )]
    capture_network: Option<String>,

    /// With --capture-network, the resource types to keep: a comma list such
    /// as xhr,fetch (the default), document, script, or all
    #[arg(long, value_name = "TYPES", requires = "capture_network")]
    capture_types: Option<String>,

    /// With --capture-network, also keep request headers that carry
    /// credentials, such as Authorization or API keys
    #[arg(long, requires = "capture_network")]
    capture_credentials: bool,

    /// Save the rendered page as a PDF to this file (- for stdout), every
    /// page of it, as the browser prints it. Needs the `browser` feature
    #[arg(long, value_name = "FILE", conflicts_with_all = ["pick", "crawl", "watch", "map", "screenshot", "format", "summarize", "cloud", "output_dir", "diff_with", "brand", "raw_html", "pdf_mode", "metadata", "query", "extract_json", "extract_prompt", "research", "include", "exclude", "only_main_content", "stdin", "file"])]
    save_pdf: Option<PathBuf>,

    /// Save the rendered page as MHTML to this file (- for stdout): one file
    /// with the page and the images, styles and frames it loaded, which a
    /// browser opens offline. Needs the `browser` feature
    #[arg(long, value_name = "FILE", conflicts_with_all = ["pick", "crawl", "watch", "map", "screenshot", "save_pdf", "paper", "landscape", "print_background", "scale", "full_page", "capture_types", "capture_credentials", "format", "summarize", "cloud", "output_dir", "diff_with", "brand", "raw_html", "pdf_mode", "metadata", "query", "extract_json", "extract_prompt", "research", "include", "exclude", "only_main_content", "stdin", "file"])]
    save_mhtml: Option<PathBuf>,

    /// Also keep every HTTP response webclaw fetches, with its request, in
    /// this WARC file, the web archive format (ISO 28500), gzipped record by
    /// record when FILE ends in .gz. Works with one URL, several, and
    /// --crawl. Records are written as they arrive, so a stopped crawl keeps
    /// what it fetched, and a FILE that exists is added to, each run starting
    /// with its own warcinfo record. Pages a browser renders are not in it:
    /// use --save-mhtml for those
    #[arg(long, value_name = "FILE", conflicts_with_all = ["cache", "cloud", "stdin", "file", "screenshot", "save_pdf", "save_mhtml", "capture_network", "full_page", "paper", "landscape", "print_background", "scale", "capture_types", "capture_credentials"])]
    warc: Option<PathBuf>,

    /// With --save-pdf, the paper size
    #[arg(long, value_enum, default_value_t = PaperArg::Letter, requires = "save_pdf")]
    paper: PaperArg,

    /// With --save-pdf, turn the paper sideways
    #[arg(long, requires = "save_pdf")]
    landscape: bool,

    /// With --save-pdf, keep background colors and images, which browsers
    /// leave out when printing
    #[arg(long, requires = "save_pdf")]
    print_background: bool,

    /// With --save-pdf, zoom the page content, from 0.1 to 2
    #[arg(
        long,
        value_name = "FACTOR",
        default_value_t = 1.0,
        value_parser = pdf_scale,
        requires = "save_pdf"
    )]
    scale: f64,

    /// CSS selectors to include (comma-separated, e.g. "article,.content")
    #[arg(long)]
    include: Option<String>,

    /// CSS selectors to exclude (comma-separated, e.g. "nav,.sidebar,footer")
    #[arg(long)]
    exclude: Option<String>,

    /// Only extract main content (article/main element)
    #[arg(long)]
    only_main_content: bool,

    /// Custom headers (repeatable, e.g. -H "Cookie: foo=bar")
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,

    /// Cookie string (shorthand for -H "Cookie: ...")
    #[arg(long)]
    cookie: Option<String>,

    /// JSON cookie file (Chrome extension format: [{name, value, domain, ...}])
    #[arg(long)]
    cookie_file: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Compare against a previous JSON snapshot
    #[arg(long)]
    diff_with: Option<String>,

    /// Watch a URL for changes. Checks at the specified interval and reports diffs.
    #[arg(long)]
    watch: bool,

    /// Watch interval in seconds [default: 300]
    #[arg(long, default_value = "300")]
    watch_interval: u64,

    /// Command to run when changes are detected (receives diff JSON on stdin)
    #[arg(long)]
    on_change: Option<String>,

    /// Webhook URL: POST a JSON payload when an operation completes.
    /// Works with crawl, batch, watch (on change), and single URL modes.
    #[arg(long, env = "WEBCLAW_WEBHOOK_URL", hide_env_values = true)]
    webhook: Option<String>,

    /// Extract brand identity (colors, fonts, logo)
    #[arg(long)]
    brand: bool,

    // -- PDF options --
    /// PDF extraction mode: auto (error on empty) or fast (return whatever text is found)
    #[arg(long, default_value = "auto")]
    pdf_mode: PdfModeArg,

    // -- Crawl options --
    /// Enable recursive crawling of same-domain links
    #[arg(long)]
    crawl: bool,

    /// Max crawl depth [default: 1]
    #[arg(long, default_value = "1")]
    depth: usize,

    /// Max pages to crawl. `0` = no limit: crawl until the site is exhausted
    /// or you interrupt it [default: 20]
    #[arg(long, default_value = "20")]
    max_pages: usize,

    /// Max concurrent requests [default: 5]
    #[arg(long, default_value = "5")]
    concurrency: usize,

    /// Minimum time between two page requests to the same host in ms, shared
    /// by all workers; the first page waits this long after robots.txt and
    /// sitemaps. With --ignore-robots, redirect hops inside one fetch are not
    /// spaced. `0` = no spacing [default: 100]
    #[arg(long, default_value = "100")]
    delay: u64,

    /// Only crawl, or list with --map, URLs matching this path prefix
    #[arg(long)]
    path_prefix: Option<String>,

    /// Glob patterns for crawl and --map URL paths to include (comma-separated, e.g. "/api/*,/guides/**")
    #[arg(long)]
    include_paths: Option<String>,

    /// Glob patterns for crawl and --map URL paths to exclude (comma-separated, e.g. "/changelog/*,/blog/*")
    #[arg(long)]
    exclude_paths: Option<String>,

    /// Path to save/resume crawl state. On Ctrl+C: saves progress. On start: resumes if file exists.
    #[arg(long)]
    crawl_state: Option<PathBuf>,

    /// Crawl URLs that robots.txt disallows, and ignore its Crawl-delay
    #[arg(long)]
    ignore_robots: bool,

    /// With --crawl --query, stop once a page rated at least 0.8 has been
    /// found and the next 8 pages rated no higher, instead of always crawling
    /// up to --max-pages
    #[arg(long, requires_all = ["crawl", "query"])]
    stop_when_answered: bool,

    /// Seed crawl frontier from sitemap discovery (robots.txt + /sitemap.xml)
    #[arg(long)]
    sitemap: bool,

    /// Discover URLs from sitemap and print them (one per line; JSON array with --format json)
    #[arg(long)]
    map: bool,

    /// Max pages for --map's crawl fallback when the sitemap is thin [default: 150]
    #[arg(long)]
    map_pages: Option<usize>,

    /// Disable --map's crawl fallback (sitemap-only discovery)
    #[arg(long)]
    no_map_crawl: bool,

    /// Cap the number of URLs --map returns, at least 1 (default: uncapped)
    #[arg(long, value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..))]
    map_limit: Option<usize>,

    /// Where --map looks for URLs, comma-separated: sitemap (the default, with
    /// the crawl fallback) and cc, the public Common Crawl index of the pages
    /// it captured on the site. E.g. sitemap,cc
    #[arg(long, value_name = "LIST", requires = "map")]
    map_source: Option<String>,

    /// With --map-source cc, also keep Common Crawl captures that were not
    /// HTML pages answering 200 (redirects, errors, PDFs, images)
    #[arg(long, requires = "map_source")]
    map_all_captures: bool,

    // -- LLM options --
    /// Extract structured JSON using LLM (pass a JSON schema string or @file)
    #[arg(long)]
    extract_json: Option<String>,

    /// Extract using natural language prompt
    #[arg(long)]
    extract_prompt: Option<String>,

    /// Summarize content using LLM (optional: number of sentences, default 3)
    #[arg(long, num_args = 0..=1, default_missing_value = "3")]
    summarize: Option<usize>,

    /// Force a specific LLM provider (ollama, openai, atlascloud, anthropic, orcarouter)
    #[arg(long, env = "WEBCLAW_LLM_PROVIDER")]
    llm_provider: Option<String>,

    /// Override the LLM model name
    #[arg(long, env = "WEBCLAW_LLM_MODEL")]
    llm_model: Option<String>,

    /// Override the LLM base URL (Ollama, OpenAI-compatible, or Anthropic-compatible)
    #[arg(long, env = "WEBCLAW_LLM_BASE_URL", hide_env_values = true)]
    llm_base_url: Option<String>,

    // -- Cloud API options --
    /// Webclaw Cloud API key for automatic fallback on bot-protected or JS-rendered sites
    #[arg(long, env = "WEBCLAW_API_KEY", hide_env_values = true)]
    api_key: Option<String>,

    /// Force all requests through the cloud API (skip local extraction)
    #[arg(long)]
    cloud: bool,

    /// Run deep research on a topic via the cloud API. Requires --api-key.
    /// Saves full result (report + sources + findings) to a JSON file.
    #[arg(long)]
    research: Option<String>,

    /// Enable deep research mode (longer, more thorough report). Used with --research.
    #[arg(long)]
    deep: bool,

    /// Output directory: save each page to a separate file instead of stdout.
    /// Works with --crawl, batch (multiple URLs), and single URL mode.
    /// Filenames are derived from URL paths (e.g. /docs/api -> docs/api.md).
    #[arg(long)]
    output_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Per-URL extraction micro-benchmark: compares raw HTML vs. the
    /// webclaw --format llm output on token count, bytes, and
    /// extraction time. Uses an approximate tokenizer (see `--help`).
    Bench {
        /// URL to benchmark.
        url: String,

        /// Emit a single JSON line instead of the ASCII table.
        /// Machine-readable shape stable across releases.
        #[arg(long)]
        json: bool,

        /// Optional path to a facts.json (same schema as the repo's
        /// benchmarks/facts.json) for a fidelity column.
        #[arg(long)]
        facts: Option<PathBuf>,
    },

    /// List all vertical extractors in the catalog.
    ///
    /// Each entry has a stable `name` (usable with `webclaw vertical <name>`),
    /// a human-friendly label, a one-line description, and the URL
    /// patterns it claims. The same data is served by `/v1/extractors`
    /// when running the REST API.
    Extractors {
        /// Emit JSON instead of a human-friendly table.
        #[arg(long)]
        json: bool,
    },

    /// Run a vertical extractor by name. Returns typed JSON with fields
    /// specific to the target site (title, price, author, rating, etc.)
    /// rather than generic markdown.
    ///
    /// Use `webclaw extractors` to see the full list. Example:
    /// `webclaw vertical reddit https://www.reddit.com/r/rust/comments/abc/`.
    Vertical {
        /// Vertical name (e.g. `reddit`, `github_repo`, `trustpilot_reviews`).
        name: String,
        /// URL to extract.
        url: String,
        /// Emit compact JSON (single line). Default is pretty-printed.
        #[arg(long)]
        raw: bool,
    },

    /// Web search via Serper.dev using YOUR OWN API key.
    ///
    /// Returns Google organic results (title, link, snippet). With
    /// `--scrape`, each result page is fetched and extracted to markdown.
    /// Get a free key at serper.dev, then pass `--serper-key` or set
    /// `SERPER_API_KEY`.
    ///
    /// Example: `webclaw search "rust async runtime" --num 5 --scrape`.
    Search {
        /// Search query.
        query: String,

        /// Serper.dev API key. Falls back to the `SERPER_API_KEY` env var.
        #[arg(long, env = "SERPER_API_KEY", hide_env_values = true)]
        serper_key: Option<String>,

        /// Number of results to return (1-10).
        #[arg(long, default_value = "5")]
        num: usize,

        /// Country code for localization (e.g. "us", "gb", "it").
        #[arg(long)]
        country: Option<String>,

        /// Language code for localization (e.g. "en", "it").
        #[arg(long)]
        lang: Option<String>,

        /// Fetch + extract each result page and include its markdown.
        #[arg(long)]
        scrape: bool,

        /// Output format: `markdown` (human-readable, default) or `json`.
        #[arg(short, long, default_value = "markdown")]
        format: OutputFormat,
    },
}

#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Markdown,
    Json,
    /// One compact JSON object per line. Crawls stream each page as it finishes.
    Jsonl,
    Text,
    Llm,
    Html,
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum RenderMode {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, ValueEnum)]
enum PaperArg {
    Letter,
    A4,
}

#[derive(Clone, ValueEnum)]
enum Browser {
    Chrome,
    ChromeMacos,
    Firefox,
    Safari,
    Edge,
    /// Safari iOS 26. Pair with a country-matched residential proxy for sites
    /// that reject non-mobile profiles.
    SafariIos,
    Random,
}

#[derive(Clone, ValueEnum, Default)]
enum PdfModeArg {
    /// Error if the PDF yields no non-whitespace text
    #[default]
    Auto,
    /// Return whatever text is found, even if empty
    Fast,
}

impl From<PdfModeArg> for PdfMode {
    fn from(arg: PdfModeArg) -> Self {
        match arg {
            PdfModeArg::Auto => PdfMode::Auto,
            PdfModeArg::Fast => PdfMode::Fast,
        }
    }
}

impl From<Browser> for BrowserProfile {
    fn from(b: Browser) -> Self {
        match b {
            Browser::Chrome => BrowserProfile::Chrome,
            Browser::ChromeMacos => BrowserProfile::ChromeMacos,
            Browser::Firefox => BrowserProfile::Firefox,
            Browser::Safari => BrowserProfile::Safari,
            Browser::Edge => BrowserProfile::Edge,
            Browser::SafariIos => BrowserProfile::SafariIos,
            Browser::Random => BrowserProfile::Random,
        }
    }
}

fn init_logging(verbose: bool) {
    // html5ever / markup5ever / selectors emit WARN on common real-world HTML
    // quirks. They are rarely actionable for CLI users, so keep them quiet by
    // default while still allowing WEBCLAW_LOG to override the filter.
    let default = "warn,html5ever=error,markup5ever=error,selectors=error";
    let filter = if verbose {
        EnvFilter::new("webclaw=debug,html5ever=error,markup5ever=error,selectors=error")
    } else {
        EnvFilter::try_from_env("WEBCLAW_LOG").unwrap_or_else(|_| EnvFilter::new(default))
    };

    // Logs go to stderr, never stdout: stdout carries the actual result
    // (markdown / JSON / URL list). A stray WARN on stdout corrupts
    // machine-readable output — e.g. `--map --format json` piped to a parser.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// Build FetchConfig from CLI flags.
///
/// `--proxy` sets a single static proxy (no rotation).
/// `--proxy-file` loads a pool of proxies and rotates per-request.
/// `--proxy` takes priority: if both are set, only the single proxy is used.
fn build_fetch_config(cli: &Cli) -> FetchConfig {
    let (proxy, proxy_pool) = if cli.proxy.is_some() {
        (cli.proxy.clone(), Vec::new())
    } else if let Some(ref path) = cli.proxy_file {
        match webclaw_fetch::parse_proxy_file(path) {
            Ok(pool) => (None, pool),
            Err(e) => {
                eprintln!("warning: {e}");
                (None, Vec::new())
            }
        }
    } else if std::path::Path::new("proxies.txt").exists() {
        // Auto-load proxies.txt from working directory if present
        match webclaw_fetch::parse_proxy_file("proxies.txt") {
            Ok(pool) if !pool.is_empty() => {
                eprintln!("loaded {} proxies from proxies.txt", pool.len());
                (None, pool)
            }
            _ => (None, Vec::new()),
        }
    } else {
        (None, Vec::new())
    };

    let mut headers = std::collections::HashMap::from([(
        "Accept-Language".to_string(),
        "en-US,en;q=0.9".to_string(),
    )]);

    // Parse -H "Key: Value" flags
    for h in &cli.headers {
        if let Some((key, val)) = h.split_once(':') {
            headers.insert(key.trim().to_string(), val.trim().to_string());
        }
    }

    // --cookie shorthand
    if let Some(ref cookie) = cli.cookie {
        headers.insert("Cookie".to_string(), cookie.clone());
    }

    // --cookie-file: parse JSON array of {name, value, domain, ...}
    if let Some(ref path) = cli.cookie_file {
        match parse_cookie_file(path) {
            Ok(cookie_str) => {
                // Merge with existing cookies if --cookie was also provided
                if let Some(existing) = headers.get("Cookie") {
                    headers.insert("Cookie".to_string(), format!("{existing}; {cookie_str}"));
                } else {
                    headers.insert("Cookie".to_string(), cookie_str);
                }
            }
            Err(e) => {
                eprintln!("error: failed to parse cookie file: {e}");
                exit(1);
            }
        }
    }

    #[allow(unused_mut)]
    let mut config = FetchConfig {
        browser: cli.browser.clone().into(),
        proxy,
        proxy_pool,
        timeout: std::time::Duration::from_secs(cli.timeout),
        pdf_mode: cli.pdf_mode.clone().into(),
        headers,
        cache: cli.cache.then(cache_dir),
        archive: cli.warc.as_deref().map(warc_archive),
        ..Default::default()
    };
    #[cfg(feature = "browser")]
    if cli.render != RenderMode::Never {
        config.render = Some(webclaw_fetch::render::RenderConfig {
            pool: render_pool(cli, &config),
            always: cli.render == RenderMode::Always,
            wait_for: cli.wait_for.clone(),
            block: cli.block.unwrap_or_default(),
            solve: cli.solve,
        });
    }
    config
}

#[cfg(feature = "browser")]
fn render_pool(cli: &Cli, config: &FetchConfig) -> Arc<webclaw_fetch::render::RenderPool> {
    use webclaw_fetch::render::RenderPool;
    static PROFILE_POOL: std::sync::OnceLock<Arc<RenderPool>> = std::sync::OnceLock::new();
    let build = || {
        let pool = RenderPool::new(
            browser_source(cli, config),
            std::time::Duration::from_secs(cli.timeout),
        );
        if let Some(cookie) = config.headers.get("Cookie") {
            for (url, _) in collect_urls(cli).unwrap_or_default() {
                pool.save_cookies(&url, cookie);
            }
        }
        Arc::new(pool)
    };
    match cli.browser_profile {
        Some(_) => PROFILE_POOL.get_or_init(build).clone(),
        None => build(),
    }
}

#[cfg(feature = "browser")]
fn browser_source(cli: &Cli, config: &FetchConfig) -> webclaw_fetch::render::BrowserSource {
    use webclaw_fetch::render::BrowserSource;
    match &cli.cdp_url {
        Some(endpoint) => BrowserSource::Connect(endpoint.clone()),
        None => BrowserSource::launch_for(config).with_profile(cli.browser_profile.clone()),
    }
}

fn check_warc(cli: &Cli) -> Result<(), String> {
    let Some(path) = &cli.warc else {
        return Ok(());
    };
    if path == Path::new("-") {
        return Err(
            "--warc needs a file: an archive is appended to, so it cannot go to stdout".into(),
        );
    }
    if cli.command.is_some() {
        return Err(
            "--warc does not reach the subcommands; run the fetch, crawl or map without one".into(),
        );
    }
    if cli.render == RenderMode::Always || cli.actions.is_some() || cli.scroll.is_some() {
        return Err("--warc keeps the HTTP responses webclaw fetches, and --render always, --actions and --scroll load the page in the browser instead; save a rendered page with --save-mhtml".into());
    }
    Ok(())
}

fn open_warc(path: &Path) -> io::Result<webclaw_fetch::warc::Writer<io::BufWriter<std::fs::File>>> {
    let gzip = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gz"));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    if !webclaw_fetch::warc::ends_whole(&mut file, gzip)? {
        return Err(io::Error::other(if gzip {
            "it ends inside a record, or is not gzipped, so records added to it would be unreadable; a run that stopped while writing can leave it so. Give --warc a new file"
        } else {
            "it ends inside a record, or is not a WARC file, so records added to it would be unreadable; a run that stopped while writing can leave it so. Give --warc a new file"
        }));
    }
    webclaw_fetch::warc::Writer::new(
        io::BufWriter::new(file),
        gzip,
        concat!("webclaw/", env!("CARGO_PKG_VERSION")),
    )
}

fn warc_archive(path: &Path) -> webclaw_fetch::warc::Archive {
    static ARCHIVE: std::sync::OnceLock<webclaw_fetch::warc::Archive> = std::sync::OnceLock::new();
    ARCHIVE
        .get_or_init(|| {
            let fail = |e: io::Error| -> ! {
                eprintln!("error: cannot write {}: {e}", path.display());
                exit(1);
            };
            let writer = open_warc(path).unwrap_or_else(|e| fail(e));
            // ponytail: one lock, written on the fetching task; a writer thread if a wide crawl waits on it
            let writer = std::sync::Mutex::new(writer);
            let path = path.to_path_buf();
            webclaw_fetch::warc::Archive(Arc::new(move |exchange| {
                let written = match writer.lock() {
                    Ok(mut writer) => writer.write(exchange),
                    Err(e) => Err(io::Error::other(e.to_string())),
                };
                if let Err(e) = written {
                    eprintln!("error: cannot write {}: {e}", path.display());
                    exit(1);
                }
            }))
        })
        .clone()
}

fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WEBCLAW_CACHE_DIR") {
        return dir.into();
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".cache").join("webclaw")
}

async fn close_browser(config: &FetchConfig) {
    #[cfg(feature = "browser")]
    if let Some(render) = &config.render {
        render.pool.close().await;
    }
    let _ = config;
}

fn pdf_scale(text: &str) -> Result<f64, String> {
    text.parse()
        .ok()
        .filter(|scale| (0.1..=2.0).contains(scale))
        .ok_or_else(|| format!("{text} is not a number from 0.1 to 2"))
}

/// Parse a JSON cookie file (Chrome extension format) into a Cookie header string.
/// Supports: [{name, value, domain, path, secure, httpOnly, expirationDate, ...}]
fn parse_cookie_file(path: &str) -> Result<String, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let cookies: Vec<serde_json::Value> =
        serde_json::from_str(&content).map_err(|e| format!("invalid JSON: {e}"))?;

    let pairs: Vec<String> = cookies
        .iter()
        .filter_map(|c| {
            let name = c.get("name")?.as_str()?;
            let value = c.get("value")?.as_str()?;
            Some(format!("{name}={value}"))
        })
        .collect();

    if pairs.is_empty() {
        return Err("no cookies found in file".to_string());
    }

    Ok(pairs.join("; "))
}

fn build_extraction_options(cli: &Cli) -> ExtractionOptions {
    ExtractionOptions {
        include_selectors: cli
            .include
            .as_deref()
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        exclude_selectors: cli
            .exclude
            .as_deref()
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        only_main_content: cli.only_main_content,
        include_raw_html: cli.raw_html || matches!(cli.format, OutputFormat::Html),
    }
}

/// Normalize a URL: prepend `https://` if no scheme is present.
fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

/// M14: detect URLs that look truncated by the shell (e.g. an unquoted URL
/// that the shell split on `&` or `?`). Returns `true` when:
///   - the URL ends with `&` (a trailing param separator suggests the next
///     param was lopped off), OR
///   - the URL contains `?` but no `=` after it (a query with bare keys is
///     rare; usually a real query has at least one `=`).
///
/// Informational only — caller decides whether to warn / abort. This is a
/// heuristic; legitimate URLs with bare-key queries will trigger a false
/// positive (suppressible via `--url-encoded`).
fn looks_truncated(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.ends_with('&') {
        return true;
    }
    if let Some((_before, after_q)) = trimmed.split_once('?') {
        // Trim a trailing fragment so `?#section` etc. doesn't mask the check.
        let query_part = after_q.split('#').next().unwrap_or(after_q);
        if !query_part.contains('=') {
            return true;
        }
    }
    false
}

/// Derive a filename from a URL for `--output-dir`.
///
/// Strips the scheme/host, maps the path to a filesystem path, and appends
/// an extension matching the output format.
fn url_to_filename(raw_url: &str, format: &OutputFormat) -> String {
    let ext = match format {
        OutputFormat::Markdown | OutputFormat::Llm => "md",
        OutputFormat::Json | OutputFormat::Jsonl => "json",
        OutputFormat::Text => "txt",
        OutputFormat::Html => "html",
    };

    let parsed = url::Url::parse(raw_url);
    let (host, path, query) = match &parsed {
        Ok(u) => (
            u.host_str().unwrap_or("unknown").to_string(),
            u.path().to_string(),
            u.query().map(String::from),
        ),
        Err(_) => (String::new(), String::new(), None),
    };

    // Drop empty / "." / ".." path segments so a URL path like
    // `/../../etc/passwd` can't climb out of the output directory.
    let cleaned_path: String = path
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .collect::<Vec<_>>()
        .join("/");

    let mut stem = cleaned_path;
    if stem.is_empty() {
        // Use hostname for root URLs to avoid collisions in batch mode
        let clean_host = host.strip_prefix("www.").unwrap_or(&host);
        stem = format!("{}/index", clean_host.replace('.', "_"));
    }

    // Append query params so /p?id=123 doesn't collide with /p?id=456
    if let Some(q) = query {
        stem = format!("{stem}_{q}");
    }

    // Sanitize: keep alphanumeric, dash, underscore, dot, slash
    let sanitized: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/') {
                c
            } else {
                '_'
            }
        })
        .collect();

    format!("{sanitized}.{ext}")
}

/// Reject a caller-supplied (CSV `url,filename`) name that could escape the
/// output directory: absolute paths, drive prefixes, root, or any `..`
/// component. Returns the validated relative path on success.
fn safe_relative_filename(filename: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(filename);
    use std::path::Component;
    for comp in candidate.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("refusing path with '..' component: {filename}"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("refusing absolute output path: {filename}"));
            }
        }
    }
    if candidate.as_os_str().is_empty() {
        return Err("empty output filename".to_string());
    }
    Ok(candidate.to_path_buf())
}

/// Write extraction output to a file inside `dir`, creating parent dirs as needed.
///
/// `filename` may originate from an attacker-controlled `--urls-file`
/// (`url,filename` CSV). It is validated for traversal, and the canonical
/// destination directory is asserted to stay under the canonical output
/// directory before any write.
fn write_to_file(dir: &Path, filename: &str, content: &str) -> Result<(), String> {
    let rel = safe_relative_filename(filename)?;
    let dest = dir.join(&rel);

    std::fs::create_dir_all(dir)
        .map_err(|e| format!("failed to create directory {}: {e}", dir.display()))?;
    let base = dir
        .canonicalize()
        .map_err(|e| format!("failed to resolve output dir {}: {e}", dir.display()))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
        let canon_parent = parent
            .canonicalize()
            .map_err(|e| format!("failed to resolve {}: {e}", parent.display()))?;
        if !canon_parent.starts_with(&base) {
            return Err(format!(
                "refusing to write outside output dir: {}",
                dest.display()
            ));
        }
    }

    std::fs::write(&dest, content)
        .map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
    let word_count = content.split_whitespace().count();
    eprintln!("Saved: {} ({word_count} words)", dest.display());
    Ok(())
}

/// Get raw HTML from an extraction result, falling back to markdown if unavailable.
fn raw_html_or_markdown(result: &ExtractionResult) -> &str {
    result
        .content
        .raw_html
        .as_deref()
        .unwrap_or(&result.content.markdown)
}

/// Format an `ExtractionResult` into a string for the given output format.
fn format_output(result: &ExtractionResult, format: &OutputFormat, show_metadata: bool) -> String {
    match format {
        OutputFormat::Markdown => {
            let mut out = String::new();
            if show_metadata {
                out.push_str(&format_frontmatter(&result.metadata));
            }
            out.push_str(&result.content.markdown);
            if !result.structured_data.is_empty() {
                out.push_str("\n\n## Structured Data\n\n```json\n");
                out.push_str(
                    &serde_json::to_string_pretty(&result.structured_data).unwrap_or_default(),
                );
                out.push_str("\n```");
            }
            out
        }
        OutputFormat::Json => serde_json::to_string_pretty(result).expect("serialization failed"),
        OutputFormat::Jsonl => format!(
            "{}\n",
            serde_json::to_string(result).expect("serialization failed")
        ),
        OutputFormat::Text => result.content.plain_text.clone(),
        OutputFormat::Llm => to_llm_text(result, result.metadata.url.as_deref()),
        OutputFormat::Html => raw_html_or_markdown(result).to_string(),
    }
}

/// Collect all URLs from positional args + --urls-file, normalizing bare domains.
///
/// Returns `(url, optional_custom_filename)` pairs. Custom filenames come from
/// CSV-style lines in `--urls-file`: `url,filename`. Plain lines (no comma) get
/// `None` so the caller auto-generates the filename from the URL.
fn collect_urls(cli: &Cli) -> Result<Vec<(String, Option<String>)>, String> {
    let mut entries: Vec<(String, Option<String>)> =
        cli.urls.iter().map(|u| (normalize_url(u), None)).collect();

    if let Some(ref path) = cli.urls_file {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((url_part, name_part)) = trimmed.split_once(',') {
                let name = name_part.trim();
                let custom = if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                };
                entries.push((normalize_url(url_part.trim()), custom));
            } else {
                entries.push((normalize_url(trimmed), None));
            }
        }
    }

    Ok(entries)
}

/// When `--urls-file` supplies exactly one URL and no positional URL was given,
/// treat it like a single positional URL so a one-line file behaves identically
/// to `webclaw <url>` (stdout, `--output-dir`, cloud fallback, `--raw-html`,
/// `--diff`, LLM paths). Without this the lone file URL gives `entries.len() == 1`,
/// which the batch gates skip, and the single path then finds an empty `cli.urls`
/// and errors "no input provided". See issue #86.
fn backfill_single_file_url(urls: &mut Vec<String>, entries: &[(String, Option<String>)]) {
    if urls.is_empty() && entries.len() == 1 {
        urls.push(entries[0].0.clone());
    }
}

/// Result that can be either a local extraction or a cloud API JSON response.
enum FetchOutput {
    Local(Box<ExtractionResult>, Vec<serde_json::Value>),
    Cloud(serde_json::Value),
}

impl FetchOutput {
    /// Get the local ExtractionResult, or try to parse it from the cloud response.
    fn into_extraction(self) -> Result<ExtractionResult, String> {
        match self {
            FetchOutput::Local(r, js_results) => {
                print_script_results(&js_results);
                Ok(*r)
            }
            FetchOutput::Cloud(resp) => {
                // Cloud response has an "extraction" field with the full ExtractionResult
                resp.get("extraction")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .or_else(|| serde_json::from_value(resp.clone()).ok())
                    .ok_or_else(|| "could not parse extraction from cloud response".to_string())
            }
        }
    }
}

/// Operations that consume ExtractionResult need the complete cloud JSON payload.
fn cloud_scrape_format(cli: &Cli) -> &'static str {
    if cli.diff_with.is_some() || has_llm_flags(cli) {
        return "json";
    }
    match cli.format {
        OutputFormat::Markdown => "markdown",
        OutputFormat::Json | OutputFormat::Jsonl => "json",
        OutputFormat::Text => "text",
        OutputFormat::Llm => "llm",
        OutputFormat::Html => "rawHtml",
    }
}

/// Fetch a URL and extract content, handling PDF detection automatically.
/// Falls back to cloud API when bot protection or JS rendering is detected.
const BROWSER_BUILD: bool = cfg!(feature = "browser");
const VERSION: &str = if BROWSER_BUILD {
    concat!(env!("CARGO_PKG_VERSION"), " +browser")
} else {
    env!("CARGO_PKG_VERSION")
};

#[cfg(feature = "browser")]
fn page_actions(cli: &Cli) -> Result<Vec<webclaw_fetch::render::Action>, String> {
    let mut actions = Vec::new();
    if let Some(times) = cli.scroll {
        actions.push(webclaw_fetch::render::Action::Scroll {
            direction: webclaw_fetch::render::Direction::Down,
            amount: None,
            selector: None,
            repeat: Some(times),
        });
    }
    if let Some(json) = &cli.actions {
        let json = match json.strip_prefix('@') {
            Some(path) => {
                std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?
            }
            None => json.clone(),
        };
        actions.extend(webclaw_fetch::render::parse_actions(&json)?);
    }
    Ok(actions)
}

fn one_url_error(cli: &Cli) -> String {
    let error = "--actions and --scroll take one URL";
    match cli.urls.iter().find(|u| u.parse::<u32>().is_ok()) {
        Some(count) if cli.scroll.is_some() => {
            format!("{error}; for a scroll count write --scroll={count}")
        }
        _ => error.to_string(),
    }
}

fn print_script_results(results: &[serde_json::Value]) {
    for (i, value) in results.iter().enumerate() {
        eprintln!("js_results[{i}]: {value}");
    }
}

#[cfg(feature = "browser")]
async fn render_page(
    cli: &Cli,
    config: &FetchConfig,
    url: &str,
) -> Result<(FetchResult, Vec<serde_json::Value>), String> {
    let actions = page_actions(cli)?;
    let render = config.render.as_ref().ok_or("rendering is off")?;
    let headers: Vec<(String, String)> = config.headers.clone().into_iter().collect();
    let (page, js_results) = render
        .pool
        .render_actions(
            url,
            None,
            render.wait_for.as_deref(),
            &headers,
            render.block,
            render.solve,
            &actions,
        )
        .await
        .map_err(|e| format!("render error: {e}"))?;
    Ok((page, js_results))
}

#[cfg(not(feature = "browser"))]
async fn render_page(
    _cli: &Cli,
    _config: &FetchConfig,
    _url: &str,
) -> Result<(FetchResult, Vec<serde_json::Value>), String> {
    Err("this webclaw build has no browser support; build it with `--features browser`".into())
}

#[cfg(feature = "browser")]
async fn run_screenshot(cli: &Cli, url: &str, path: &Path) -> Result<(), String> {
    let actions = page_actions(cli)?;
    let config = build_fetch_config(cli);
    let render = config
        .render
        .as_ref()
        .ok_or("--screenshot needs the browser, so it cannot run with --render never")?;
    let jpeg = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    let headers: Vec<(String, String)> = config.headers.clone().into_iter().collect();
    let shot = render
        .pool
        .screenshot_actions(
            url,
            render.wait_for.as_deref(),
            &headers,
            webclaw_fetch::render::ScreenshotOptions {
                full_page: cli.full_page,
                jpeg,
            },
            &actions,
        )
        .await;
    close_browser(&config).await;
    let (shot, js_results) = shot.map_err(|e| format!("screenshot error: {e}"))?;
    print_script_results(&js_results);
    webclaw_fetch::FetchError::check_status(shot.status).map_err(|e| e.to_string())?;
    std::fs::write(path, &shot.image)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    eprintln!(
        "saved {} ({} bytes) of {}",
        path.display(),
        shot.image.len(),
        shot.url
    );
    Ok(())
}

#[cfg(feature = "browser")]
async fn run_capture(cli: &Cli, url: &str, pattern: &str) -> Result<(), String> {
    let actions = page_actions(cli)?;
    let capture = webclaw_fetch::render::NetworkCapture::new(
        Some(pattern),
        cli.capture_types.as_deref(),
        cli.capture_credentials,
    )?;
    let config = build_fetch_config(cli);
    let render = config
        .render
        .as_ref()
        .ok_or("--capture-network needs the browser, so it cannot run with --render never")?;
    let headers: Vec<(String, String)> = config.headers.clone().into_iter().collect();
    let captured = webclaw_fetch::with_progress(
        url,
        render.pool.capture(
            url,
            render.wait_for.as_deref(),
            &headers,
            render.block,
            render.solve,
            &actions,
            &capture,
        ),
    )
    .await;
    close_browser(&config).await;
    let webclaw_fetch::render::Captured {
        page,
        js_results,
        network,
    } = captured.map_err(|e| format!("render error: {e}"))?;
    print_script_results(&js_results);
    webclaw_fetch::FetchError::check_status(page.status).map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&network).map_err(|e| e.to_string())?
    );
    eprintln!("captured {} responses of {}", network.len(), page.url);
    Ok(())
}

#[cfg(not(feature = "browser"))]
async fn run_capture(_cli: &Cli, _url: &str, _pattern: &str) -> Result<(), String> {
    Err("this webclaw build has no browser support; build it with `--features browser`".into())
}

#[cfg(not(feature = "browser"))]
async fn run_screenshot(_cli: &Cli, _url: &str, _path: &Path) -> Result<(), String> {
    Err("this webclaw build has no browser support; build it with `--features browser`".into())
}

#[cfg(feature = "browser")]
async fn run_save(cli: &Cli, url: &str, path: &Path, pdf: bool) -> Result<(), String> {
    use webclaw_fetch::render::{Paper, PdfOptions};
    let (flag, kind) = if pdf {
        ("--save-pdf", "PDF")
    } else {
        ("--save-mhtml", "MHTML")
    };
    let actions = page_actions(cli)?;
    let config = build_fetch_config(cli);
    let render = config.render.as_ref().ok_or(format!(
        "{flag} needs the browser, so it cannot run with --render never"
    ))?;
    let headers: Vec<(String, String)> = config.headers.clone().into_iter().collect();
    let to_stdout = path == Path::new("-");
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut out: Box<dyn std::io::Write + Send> = if to_stdout {
        Box::new(std::io::stdout())
    } else {
        Box::new(
            std::fs::File::create(&part)
                .map_err(|e| format!("cannot write {}: {e}", part.display()))?,
        )
    };
    let wait_for = render.wait_for.as_deref();
    let printed = if pdf {
        let mut options = PdfOptions::default();
        options.paper = match cli.paper {
            PaperArg::Letter => Paper::Letter,
            PaperArg::A4 => Paper::A4,
        };
        options.landscape = cli.landscape;
        options.print_background = cli.print_background;
        options.scale = cli.scale;
        render
            .pool
            .pdf_actions(url, wait_for, &headers, options, &actions, &mut out)
            .await
            .map(|(p, js)| ((p.bytes, p.status, p.url), js))
    } else {
        render
            .pool
            .mhtml_actions(url, wait_for, &headers, &actions, &mut out)
            .await
            .map(|(m, js)| ((m.bytes, m.status, m.url), js))
    };
    close_browser(&config).await;
    let printed = printed
        .map_err(|e| match e.to_string() {
            e if e.contains("the render timeout covers") => {
                format!("{kind} error: {e}; raise it with --timeout")
            }
            e => format!("{kind} error: {e}"),
        })
        .and_then(|(printed, js_results)| {
            print_script_results(&js_results);
            webclaw_fetch::FetchError::check_status(printed.1)
                .map(|()| printed)
                .map_err(|e| e.to_string())
        });
    let flushed = out.flush();
    drop(out);
    let printed = printed
        .and_then(|printed| {
            flushed
                .map(|()| printed)
                .map_err(|e| format!("cannot write {}: {e}", part.display()))
        })
        .and_then(|printed| {
            if !to_stdout {
                std::fs::rename(&part, path)
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            }
            Ok(printed)
        });
    let printed = match printed {
        Ok(printed) => printed,
        Err(e) => {
            if !to_stdout {
                let _ = std::fs::remove_file(&part);
            }
            return Err(e);
        }
    };
    let target = if to_stdout {
        "stdout".to_string()
    } else {
        path.display().to_string()
    };
    eprintln!("saved {target} ({} bytes) of {}", printed.0, printed.2);
    Ok(())
}

#[cfg(not(feature = "browser"))]
async fn run_save(_cli: &Cli, _url: &str, _path: &Path, _pdf: bool) -> Result<(), String> {
    Err("this webclaw build has no browser support; build it with `--features browser`".into())
}

async fn fetch_and_extract(cli: &Cli) -> Result<FetchOutput, String> {
    if cli.stdin {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        let options = build_extraction_options(cli);
        return extract_with_options(&buf, None, &options)
            .map(|r| FetchOutput::Local(Box::new(r), Vec::new()))
            .map_err(|e| format!("extraction error: {e}"));
    }

    if let Some(ref path) = cli.file {
        let html =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?;
        let options = build_extraction_options(cli);
        return extract_with_options(&html, None, &options)
            .map(|r| FetchOutput::Local(Box::new(r), Vec::new()))
            .map_err(|e| format!("extraction error: {e}"));
    }

    let raw_url = cli
        .urls
        .first()
        .ok_or("no input provided -- pass a URL, --file, or --stdin")?;
    if !cli.url_encoded && looks_truncated(raw_url) {
        eprintln!(
            "# webclaw: warning: URL looks truncated (ends with '&' or '?'); did the shell split it? Quote the URL or use --url-encoded."
        );
    }
    let url = normalize_url(raw_url);
    let url = url.as_str();

    let cloud_client = webclaw_fetch::cloud::CloudClient::new(cli.api_key.as_deref());

    if cli.cloud {
        let c =
            cloud_client.ok_or("--cloud requires WEBCLAW_API_KEY (set via env or --api-key)")?;
        let options = build_extraction_options(cli);
        let format_str = cloud_scrape_format(cli);
        let resp = c
            .scrape(
                url,
                &[format_str],
                &options.include_selectors,
                &options.exclude_selectors,
                options.only_main_content,
            )
            .await?;
        return Ok(FetchOutput::Cloud(resp));
    }

    let config = build_fetch_config(cli);
    let client = FetchClient::new(config.clone()).map_err(|e| format!("client error: {e}"))?;
    let options = build_extraction_options(cli);
    let mut js_results = Vec::new();
    let extracted = if cli.render == RenderMode::Always {
        let rendered = webclaw_fetch::with_progress(url, render_page(cli, &config, url)).await;
        close_browser(&config).await;
        let (page, values) = rendered?;
        js_results = values;
        client.extract(&page, &options)
    } else {
        let fetched = webclaw_fetch::with_progress(url, client.fetch_smart(url))
            .await
            .map_err(|e| format!("fetch error: {e}"))?;
        let (_, extracted) =
            webclaw_fetch::with_progress(url, client.extract_page(fetched, &options)).await;
        close_browser(&config).await;
        extracted
    };
    let result = match extracted {
        Ok(result) => result,
        Err(
            error @ (webclaw_fetch::FetchError::Content(_)
            | webclaw_fetch::FetchError::UpstreamStatus(_)),
        ) => {
            if let Some(ref c) = cloud_client {
                let format = cloud_scrape_format(cli);
                let response = c
                    .scrape(
                        url,
                        &[format],
                        &options.include_selectors,
                        &options.exclude_selectors,
                        options.only_main_content,
                    )
                    .await?;
                return Ok(FetchOutput::Cloud(response));
            }
            return Err(error.to_string());
        }
        Err(error) => return Err(format!("fetch error: {error}")),
    };

    let reason = detect_empty(&result);
    if !matches!(reason, EmptyReason::None) {
        if let Some(ref c) = cloud_client {
            eprintln!("\x1b[36minfo:\x1b[0m falling back to cloud API...");
            let format_str = cloud_scrape_format(cli);
            match c
                .scrape(
                    url,
                    &[format_str],
                    &options.include_selectors,
                    &options.exclude_selectors,
                    options.only_main_content,
                )
                .await
            {
                Ok(resp) => return Ok(FetchOutput::Cloud(resp)),
                Err(e) => {
                    eprintln!("\x1b[33mwarning:\x1b[0m cloud fallback failed: {e}");
                }
            }
        }
        warn_empty(url, &reason);
    }

    Ok(FetchOutput::Local(Box::new(result), js_results))
}

/// Fetch raw HTML from a URL (no extraction). Used for --raw-html and brand extraction.
async fn fetch_html(cli: &Cli) -> Result<FetchResult, String> {
    if cli.stdin {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        return Ok(FetchResult {
            document_bytes: None,
            html: buf,
            url: String::new(),
            status: 200,
            headers: Default::default(),
            elapsed: Default::default(),
        });
    }

    if let Some(ref path) = cli.file {
        let html =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?;
        return Ok(FetchResult {
            document_bytes: None,
            html,
            url: String::new(),
            status: 200,
            headers: Default::default(),
            elapsed: Default::default(),
        });
    }

    let raw_url = cli
        .urls
        .first()
        .ok_or("no input provided -- pass a URL, --file, or --stdin")?;
    let url = normalize_url(raw_url);

    let client =
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?;
    client
        .fetch(&url)
        .await
        .map_err(|e| format!("fetch error: {e}"))
}

async fn fetch_page(cli: &Cli) -> Result<FetchResult, String> {
    let page = if cli.stdin || cli.file.is_some() {
        fetch_html(cli).await?
    } else {
        let url = normalize_url(
            cli.urls
                .first()
                .ok_or("no input provided -- pass a URL, --file, or --stdin")?,
        );
        let config = build_fetch_config(cli);
        let client = FetchClient::new(config.clone()).map_err(|e| format!("client error: {e}"))?;
        let page = if cli.render == RenderMode::Always {
            render_page(cli, &config, &url)
                .await
                .map(|(page, js_results)| {
                    print_script_results(&js_results);
                    page
                })
        } else {
            match client.fetch_smart(&url).await {
                Ok(fetched) => Ok(client
                    .extract_page(fetched, &build_extraction_options(cli))
                    .await
                    .0),
                Err(e) => Err(format!("fetch error: {e}")),
            }
        };
        close_browser(&config).await;
        page?
    };
    webclaw_fetch::FetchError::check_status(page.status).map_err(|e| e.to_string())?;
    Ok(page)
}

async fn run_tables(cli: &Cli) -> Result<(), String> {
    let page = fetch_page(cli).await?;
    let json = serde_json::to_string_pretty(&webclaw_core::tables(&page.html));
    println!("{}", json.map_err(|e| e.to_string())?);
    Ok(())
}

async fn run_select(cli: &Cli) -> Result<(), String> {
    let text = match (&cli.find_text, &cli.find_regex) {
        (Some(text), _) => Some(webclaw_core::TextMatch::text(
            text,
            cli.exact,
            cli.case_sensitive,
        )),
        (None, Some(regex)) => Some(webclaw_core::TextMatch::regex(regex, cli.case_sensitive)),
        (None, None) => None,
    }
    .transpose()
    .map_err(|e| e.to_string())?;
    if text.is_some() && !cli.similar && !cli.fields.is_empty() {
        return Err("--field works with --select, --xpath or --similar".into());
    }
    let fields = cli
        .fields
        .iter()
        .map(|field| {
            field
                .split_once('=')
                .map(|(name, css)| (name.trim().to_string(), css.trim().to_string()))
                .ok_or_else(|| format!("--field needs NAME=SELECTOR, got `{field}`"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let page = fetch_page(cli).await?;
    let url = (!page.url.is_empty()).then_some(page.url.as_str());
    let html = page.html.as_str();
    let json = match (&cli.select, &cli.xpath, &text) {
        _ if cli.similar => {
            let example = match (&cli.select, &cli.xpath, &text) {
                (Some(css), _, _) => webclaw_core::FindExample::Css(css),
                (_, Some(xpath), _) => webclaw_core::FindExample::XPath(xpath),
                (_, _, Some(text)) => webclaw_core::FindExample::Text(text),
                _ => return Err("--similar needs an example".into()),
            };
            let similar = webclaw_core::find_similar(html, example)
                .map_err(|e| e.to_string())?
                .ok_or("nothing on the page matches the example")?;
            if fields.is_empty() {
                serde_json::to_string_pretty(&similar)
            } else {
                let records =
                    webclaw_core::select_records(html, url, &similar.selector, &fields)
                        .map_err(|e| format!("with --similar, --field takes CSS selectors: {e}"))?;
                serde_json::to_string_pretty(
                    &serde_json::json!({"selector": similar.selector, "records": records}),
                )
            }
        }
        (_, _, Some(text)) => serde_json::to_string_pretty(&webclaw_core::find_text(html, text)),
        (Some(css), _, _) if fields.is_empty() => serde_json::to_string_pretty(
            &webclaw_core::select(html, url, css).map_err(|e| e.to_string())?,
        ),
        (Some(css), _, _) => serde_json::to_string_pretty(
            &webclaw_core::select_records(html, url, css, &fields).map_err(|e| e.to_string())?,
        ),
        (_, Some(xpath), _) if fields.is_empty() => serde_json::to_string_pretty(
            &webclaw_core::select_xpath(html, url, xpath).map_err(|e| e.to_string())?,
        ),
        (_, Some(xpath), _) => serde_json::to_string_pretty(
            &webclaw_core::select_xpath_records(html, url, xpath, &fields)
                .map_err(|e| e.to_string())?,
        ),
        (None, None, None) => return Err("nothing to pick".into()),
    };
    println!("{}", json.map_err(|e| e.to_string())?);
    Ok(())
}

/// Fetch external stylesheets referenced in HTML and inject them as `<style>` blocks.
/// This allows brand extraction to see colors/fonts from external CSS files.
async fn enrich_html_with_stylesheets(html: &str, base_url: &str) -> String {
    let base = match url::Url::parse(base_url) {
        Ok(u) => u,
        Err(_) => return html.to_string(),
    };

    let re = regex::Regex::new(
        r#"<link[^>]+rel=["']stylesheet["'][^>]+href=["']([^"']+)["']|<link[^>]+href=["']([^"']+)["'][^>]+rel=["']stylesheet["']"#
    ).expect("regex literal is valid");

    let hrefs: Vec<String> = re
        .captures_iter(html)
        .filter_map(|cap| {
            let href = cap.get(1).or(cap.get(2))?;
            Some(
                base.join(href.as_str())
                    .map(|u| u.to_string())
                    .unwrap_or_else(|_| href.as_str().to_string()),
            )
        })
        .take(10)
        .collect();

    if hrefs.is_empty() {
        return html.to_string();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default();

    let mut extra_css = String::new();
    for href in &hrefs {
        if webclaw_fetch::url_security::validate_public_http_url(href)
            .await
            .is_err()
        {
            continue;
        }
        if let Ok(resp) = client.get(href).send().await
            && resp.status().is_success()
            && let Ok(body) = resp.text().await
            && !body.trim_start().starts_with("<!")
            && body.len() < 2_000_000
        {
            extra_css.push_str("\n<style>\n");
            extra_css.push_str(&body);
            extra_css.push_str("\n</style>\n");
        }
    }

    if extra_css.is_empty() {
        return html.to_string();
    }

    if let Some(pos) = html.to_lowercase().find("</head>") {
        let mut enriched = String::with_capacity(html.len() + extra_css.len());
        enriched.push_str(&html[..pos]);
        enriched.push_str(&extra_css);
        enriched.push_str(&html[pos..]);
        enriched
    } else {
        format!("{extra_css}{html}")
    }
}

fn format_frontmatter(meta: &Metadata) -> String {
    let mut lines = vec!["---".to_string()];

    if let Some(title) = &meta.title {
        lines.push(format!("title: \"{title}\""));
    }
    if let Some(author) = &meta.author {
        lines.push(format!("author: \"{author}\""));
    }
    if let Some(date) = &meta.published_date {
        lines.push(format!("date: \"{date}\""));
    }
    if let Some(url) = &meta.url {
        lines.push(format!("source: \"{url}\""));
    }
    if meta.word_count > 0 {
        lines.push(format!("word_count: {}", meta.word_count));
    }

    lines.push("---".to_string());
    lines.push(String::new()); // blank line after frontmatter
    lines.join("\n")
}

fn with_js_results(
    result: &ExtractionResult,
    js_results: &[serde_json::Value],
    pretty: bool,
) -> String {
    let mut value = serde_json::to_value(result).unwrap_or_default();
    value["js_results"] = serde_json::Value::from(js_results.to_vec());
    if pretty {
        serde_json::to_string_pretty(&value).unwrap_or_default()
    } else {
        value.to_string()
    }
}

fn print_output(result: &ExtractionResult, format: &OutputFormat, show_metadata: bool) {
    match format {
        OutputFormat::Jsonl => {
            println!(
                "{}",
                serde_json::to_string(result).expect("serialization failed")
            );
        }
        OutputFormat::Markdown => {
            if show_metadata {
                print!("{}", format_frontmatter(&result.metadata));
            }
            println!("{}", result.content.markdown);
            if !result.structured_data.is_empty() {
                println!(
                    "\n## Structured Data\n\n```json\n{}\n```",
                    serde_json::to_string_pretty(&result.structured_data).unwrap_or_default()
                );
            }
        }
        OutputFormat::Json => {
            // serde_json::to_string_pretty won't fail on our types
            println!(
                "{}",
                serde_json::to_string_pretty(result).expect("serialization failed")
            );
        }
        OutputFormat::Text => {
            println!("{}", result.content.plain_text);
        }
        OutputFormat::Llm => {
            println!("{}", to_llm_text(result, result.metadata.url.as_deref()));
        }
        OutputFormat::Html => {
            println!("{}", raw_html_or_markdown(result));
        }
    }
}

fn chunk_sizes(cli: &Cli) -> Option<(u32, u32)> {
    cli.chunk.map(|size| (size, cli.chunk_overlap))
}

fn check_markdown_flags(cli: &Cli) -> Result<(), String> {
    let markdown_less = matches!(
        cli.format,
        OutputFormat::Text | OutputFormat::Llm | OutputFormat::Html
    );
    if cli.chunk.is_some() && markdown_less {
        return Err(
            "--chunk prints markdown chunks as JSON lines, or one JSON array with -f json".into(),
        );
    }
    if cli.citations && markdown_less {
        return Err("--citations rewrites markdown: use -f markdown, json or jsonl".into());
    }
    Ok(())
}

fn cite_pages<'a>(pages: impl IntoIterator<Item = &'a mut ExtractionResult>, together: bool) {
    let mut last = 0;
    for page in pages {
        let (markdown, high) =
            to_citations_after(&page.content.markdown, page.metadata.url.as_deref(), last);
        page.content.markdown = markdown;
        if together {
            last = high;
        }
    }
}

fn printed_together(cli: &Cli) -> bool {
    matches!(cli.format, OutputFormat::Markdown) && cli.output_dir.is_none()
}

fn chunk_page(
    markdown: &str,
    url: Option<&str>,
    (size, overlap): (u32, u32),
) -> Vec<webclaw_core::Chunk> {
    let mut chunks = webclaw_core::chunk(markdown, size as usize, overlap as usize);
    for chunk in &mut chunks {
        chunk.url = url.map(String::from);
    }
    chunks
}

fn print_chunks(chunks: &[webclaw_core::Chunk], format: &OutputFormat) {
    if matches!(format, OutputFormat::Json) {
        println!(
            "{}",
            serde_json::to_string_pretty(chunks).expect("serialization failed")
        );
        return;
    }
    for chunk in chunks {
        println!(
            "{}",
            serde_json::to_string(chunk).expect("serialization failed")
        );
    }
}

fn cloud_text<'a>(resp: &'a serde_json::Value, field: &str, legacy: &str) -> Option<&'a str> {
    resp.get(field)
        .and_then(|v| v.as_str())
        .or_else(|| resp.get("content")?.get(legacy)?.as_str())
}

/// Print cloud API response in the requested format.
fn print_cloud_output(resp: &serde_json::Value, format: &OutputFormat) {
    match format {
        OutputFormat::Jsonl => println!("{resp}"),
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(resp).expect("serialization failed")
            );
        }
        OutputFormat::Markdown => {
            // Cloud response has content.markdown
            if let Some(md) = resp
                .get("content")
                .and_then(|c| c.get("markdown"))
                .and_then(|m| m.as_str())
            {
                println!("{md}");
            } else if let Some(md) = resp.get("markdown").and_then(|m| m.as_str()) {
                println!("{md}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(resp).expect("serialization failed")
                );
            }
        }
        OutputFormat::Text => {
            if let Some(txt) = cloud_text(resp, "text", "plain_text") {
                println!("{txt}");
            } else {
                // Fallback to markdown or raw JSON
                print_cloud_output(resp, &OutputFormat::Markdown);
            }
        }
        OutputFormat::Llm => {
            if let Some(llm) = cloud_text(resp, "llm", "llm_text") {
                println!("{llm}");
            } else {
                print_cloud_output(resp, &OutputFormat::Markdown);
            }
        }
        OutputFormat::Html => {
            if let Some(html) = cloud_text(resp, "rawHtml", "raw_html") {
                println!("{html}");
            } else {
                print_cloud_output(resp, &OutputFormat::Markdown);
            }
        }
    }
}

fn print_diff_output(diff: &ContentDiff, format: &OutputFormat) {
    match format {
        OutputFormat::Jsonl => {
            println!(
                "{}",
                serde_json::to_string(diff).expect("serialization failed")
            );
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(diff).expect("serialization failed")
            );
        }
        // For markdown/text/llm, show a human-readable summary
        _ => {
            println!("Status: {:?}", diff.status);
            println!("Word count delta: {:+}", diff.word_count_delta);

            if !diff.metadata_changes.is_empty() {
                println!("\nMetadata changes:");
                for change in &diff.metadata_changes {
                    println!(
                        "  {}: {} -> {}",
                        change.field,
                        change.old.as_deref().unwrap_or("(none)"),
                        change.new.as_deref().unwrap_or("(none)"),
                    );
                }
            }

            if !diff.links_added.is_empty() {
                println!("\nLinks added:");
                for link in &diff.links_added {
                    println!("  + {} ({})", link.href, link.text);
                }
            }

            if !diff.links_removed.is_empty() {
                println!("\nLinks removed:");
                for link in &diff.links_removed {
                    println!("  - {} ({})", link.href, link.text);
                }
            }

            if let Some(ref text_diff) = diff.text_diff {
                println!("\n{text_diff}");
            }
        }
    }
}

fn print_crawl_output(result: &CrawlResult, format: &OutputFormat, show_metadata: bool) {
    match format {
        OutputFormat::Jsonl => {
            for page in &result.pages {
                println!(
                    "{}",
                    serde_json::to_string(page).expect("serialization failed")
                );
            }
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).expect("serialization failed")
            );
        }
        OutputFormat::Markdown => {
            for page in &result.pages {
                let Some(ref extraction) = page.extraction else {
                    continue;
                };
                println!("---");
                println!("# Page: {}\n", page.url);
                if show_metadata {
                    print!("{}", format_frontmatter(&extraction.metadata));
                }
                println!("{}", extraction.content.markdown);
                println!();
            }
        }
        OutputFormat::Text => {
            for page in &result.pages {
                let Some(ref extraction) = page.extraction else {
                    continue;
                };
                println!("---");
                println!("# Page: {}\n", page.url);
                println!("{}", extraction.content.plain_text);
                println!();
            }
        }
        OutputFormat::Llm => {
            for page in &result.pages {
                let Some(ref extraction) = page.extraction else {
                    continue;
                };
                println!("---");
                println!("{}", to_llm_text(extraction, Some(page.url.as_str())));
                println!();
            }
        }
        OutputFormat::Html => {
            for page in &result.pages {
                let Some(ref extraction) = page.extraction else {
                    continue;
                };
                println!("---");
                println!("<!-- Page: {} -->\n", page.url);
                println!("{}", raw_html_or_markdown(extraction));
                println!();
            }
        }
    }
}

fn print_batch_output(results: &[BatchExtractResult], format: &OutputFormat, show_metadata: bool) {
    match format {
        OutputFormat::Jsonl => {
            for r in results {
                let line = match &r.result {
                    Ok(extraction) => serde_json::json!({ "url": r.url, "result": extraction }),
                    Err(e) => serde_json::json!({ "url": r.url, "error": e.to_string() }),
                };
                println!("{line}");
            }
        }
        OutputFormat::Json => {
            // Build a JSON array of {url, result?, error?} objects
            let entries: Vec<serde_json::Value> = results
                .iter()
                .map(|r| match &r.result {
                    Ok(extraction) => serde_json::json!({
                        "url": r.url,
                        "result": extraction,
                    }),
                    Err(e) => serde_json::json!({
                        "url": r.url,
                        "error": e.to_string(),
                    }),
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&entries).expect("serialization failed")
            );
        }
        OutputFormat::Markdown => {
            for r in results {
                match &r.result {
                    Ok(extraction) => {
                        println!("---");
                        println!("# {}\n", r.url);
                        if show_metadata {
                            print!("{}", format_frontmatter(&extraction.metadata));
                        }
                        println!("{}", extraction.content.markdown);
                        println!();
                    }
                    Err(e) => {
                        eprintln!("error: {} -- {}", r.url, e);
                    }
                }
            }
        }
        OutputFormat::Text => {
            for r in results {
                match &r.result {
                    Ok(extraction) => {
                        println!("---");
                        println!("# {}\n", r.url);
                        println!("{}", extraction.content.plain_text);
                        println!();
                    }
                    Err(e) => {
                        eprintln!("error: {} -- {}", r.url, e);
                    }
                }
            }
        }
        OutputFormat::Llm => {
            for r in results {
                match &r.result {
                    Ok(extraction) => {
                        println!("---");
                        println!("{}", to_llm_text(extraction, Some(r.url.as_str())));
                        println!();
                    }
                    Err(e) => {
                        eprintln!("error: {} -- {}", r.url, e);
                    }
                }
            }
        }
        OutputFormat::Html => {
            for r in results {
                match &r.result {
                    Ok(extraction) => {
                        println!("---");
                        println!("<!-- {} -->\n", r.url);
                        println!("{}", raw_html_or_markdown(extraction));
                        println!();
                    }
                    Err(e) => {
                        eprintln!("error: {} -- {}", r.url, e);
                    }
                }
            }
        }
    }
}

fn path_globs(list: Option<&str>) -> Vec<String> {
    list.map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default()
}

fn print_map_output(entries: &[SitemapEntry], format: &OutputFormat) {
    match format {
        OutputFormat::Jsonl => {
            for entry in entries {
                println!(
                    "{}",
                    serde_json::to_string(entry).expect("serialization failed")
                );
            }
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(entries).expect("serialization failed")
            );
        }
        _ => {
            for entry in entries {
                println!("{}", entry.url);
            }
        }
    }
}

/// Format a streaming progress line for a completed page.
fn format_progress(page: &PageResult, index: usize, max_pages: Option<usize>) -> String {
    let status = if page.error.is_some() { "ERR" } else { "OK " };
    let timing = format!("{}ms", page.elapsed.as_millis());
    let detail = if let Some(ref extraction) = page.extraction {
        format!(", {} words", extraction.metadata.word_count)
    } else if let Some(ref err) = page.error {
        format!(" ({err})")
    } else {
        String::new()
    };
    // Uncapped crawls have no denominator to show, so print a running count.
    let of = max_pages.map_or(String::new(), |m| format!("/{m}"));
    format!("[{index}{of}] {status} {} ({timing}{detail})", page.url)
}

fn report_page_problems(page: &PageResult) {
    if let Some(ref err) = page.error {
        eprintln!("error: {} -- {}", page.url, err);
    } else if let Some(ref extraction) = page.extraction {
        let reason = detect_empty(extraction);
        if !matches!(reason, EmptyReason::None) {
            warn_empty(&page.url, &reason);
        }
    }
}

async fn run_crawl(cli: &Cli) -> Result<(), String> {
    let url = cli
        .urls
        .first()
        .ok_or("--crawl requires a URL argument")
        .map(|u| normalize_url(u))?;
    let url = url.as_str();

    if cli.file.is_some() || cli.stdin {
        return Err("--crawl cannot be used with --file or --stdin".into());
    }

    let include_patterns = path_globs(cli.include_paths.as_deref());
    let exclude_patterns = path_globs(cli.exclude_paths.as_deref());

    let (progress_tx, mut progress_rx) = tokio::sync::broadcast::channel::<PageResult>(4096);
    let stream_jsonl = (matches!(cli.format, OutputFormat::Jsonl)
        || (cli.chunk.is_some() && !matches!(cli.format, OutputFormat::Json)))
        && cli.output_dir.is_none();
    let chunk_cli = chunk_sizes(cli);
    let streamed_urls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keep_urls = cli.webhook.is_some();
    let citations = cli.citations;
    let on_page = stream_jsonl.then(|| {
        let streamed_urls = Arc::clone(&streamed_urls);
        PageSink(Arc::new(move |page: &PageResult| {
            match (chunk_cli, &page.extraction) {
                (Some(sizes), Some(extraction)) => print_chunks(
                    &chunk_page(&extraction.content.markdown, Some(&page.url), sizes),
                    &OutputFormat::Jsonl,
                ),
                (Some(_), None) => {}
                (None, Some(_)) if citations => {
                    let mut page = page.clone();
                    cite_pages(page.extraction.iter_mut(), false);
                    println!(
                        "{}",
                        serde_json::to_string(&page).expect("serialization failed")
                    );
                }
                (None, _) => println!(
                    "{}",
                    serde_json::to_string(page).expect("serialization failed")
                ),
            }
            report_page_problems(page);
            if keep_urls {
                streamed_urls
                    .lock()
                    .expect("streamed_urls lock is not poisoned")
                    .push(page.url.clone());
            }
        }))
    });

    let cancel_flag = Arc::new(AtomicBool::new(false));

    let state_path = cli.crawl_state.clone();
    if state_path.is_some() {
        let flag = Arc::clone(&cancel_flag);
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            flag.store(true, Ordering::Relaxed);
            eprintln!("\nCtrl+C received, saving crawl state...");
        });
    }

    let fetch = build_fetch_config(cli);
    let config = CrawlConfig {
        fetch: fetch.clone(),
        max_depth: cli.depth,
        max_pages: (cli.max_pages > 0).then_some(cli.max_pages),
        stream_only: stream_jsonl,
        concurrency: cli.concurrency,
        delay: std::time::Duration::from_millis(cli.delay),
        path_prefix: cli.path_prefix.clone(),
        use_sitemap: cli.sitemap,
        include_patterns,
        exclude_patterns,
        progress_tx: Some(progress_tx),
        cancel_flag: Some(Arc::clone(&cancel_flag)),
        allow_subdomains: false,
        allow_external_links: false,
        respect_robots: !cli.ignore_robots,
        checkpoint: state_path.clone(),
        on_page,
        query: cli.query.clone(),
        stop_when_answered: cli.stop_when_answered,
    };

    let resume_state = state_path
        .as_ref()
        .and_then(|p| Crawler::load_state(p))
        .inspect(|s| {
            eprintln!(
                "Resuming crawl: {} pages already visited, {} URLs in frontier",
                s.visited.len(),
                s.frontier.len(),
            );
        });

    let max_pages = (cli.max_pages > 0).then_some(cli.max_pages);
    let completed_offset = resume_state.as_ref().map_or(0, |s| s.completed_pages);

    let progress_handle = tokio::spawn(async move {
        let mut count = completed_offset;
        loop {
            match progress_rx.recv().await {
                Ok(page) => {
                    count += 1;
                    eprintln!("{}", format_progress(&page, count, max_pages));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("warning: output fell behind the crawl, {n} pages were not printed");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let crawler = Crawler::new(url, config).map_err(|e| format!("crawler error: {e}"))?;
    let mut result = crawler.crawl(url, resume_state).await;
    if cli.query.is_some() {
        result.pages.sort_by(|a, b| {
            b.relevance
                .unwrap_or(0.0)
                .total_cmp(&a.relevance.unwrap_or(0.0))
        });
    }
    if cli.citations {
        cite_pages(
            result
                .pages
                .iter_mut()
                .filter_map(|p| p.extraction.as_mut()),
            printed_together(cli),
        );
    }

    drop(crawler);
    close_browser(&fetch).await;
    let _ = progress_handle.await;

    let was_cancelled = cancel_flag.load(Ordering::Relaxed);
    if was_cancelled {
        if let Some(ref path) = state_path {
            eprintln!(
                "Crawl state saved to {} ({} pages completed). Resume with --crawl-state {}",
                path.display(),
                completed_offset + result.total,
                path.display(),
            );
        }
    } else if let Some(ref path) = state_path
        && path.exists()
    {
        let _ = std::fs::remove_file(path);
    }

    result.pages.iter().for_each(report_page_problems);

    if let Some(ref dir) = cli.output_dir {
        let mut saved = 0usize;
        for page in &result.pages {
            if let Some(ref extraction) = page.extraction {
                let filename = url_to_filename(&page.url, &cli.format);
                let content = format_output(extraction, &cli.format, cli.metadata);
                write_to_file(dir, &filename, &content)?;
                saved += 1;
            }
        }
        eprintln!("Saved {saved} files to {}", dir.display());
    } else if let Some(sizes) = chunk_sizes(cli) {
        if !stream_jsonl {
            let chunks: Vec<_> = result
                .pages
                .iter()
                .filter_map(|p| {
                    Some(chunk_page(
                        &p.extraction.as_ref()?.content.markdown,
                        Some(&p.url),
                        sizes,
                    ))
                })
                .flatten()
                .collect();
            print_chunks(&chunks, &cli.format);
        }
    } else {
        print_crawl_output(&result, &cli.format, cli.metadata);
    }

    eprintln!(
        "Crawled {} pages ({} ok, {} errors) in {:.1}s",
        result.total, result.ok, result.errors, result.elapsed_secs,
    );
    if result.skipped_by_robots > 0 {
        eprintln!(
            "Skipped {} URLs disallowed by robots.txt (--ignore-robots to crawl them)",
            result.skipped_by_robots
        );
    }

    if let Some(ref webhook_url) = cli.webhook {
        let streamed = streamed_urls
            .lock()
            .expect("streamed_urls lock is not poisoned")
            .clone();
        let urls: Vec<&str> = if stream_jsonl {
            streamed.iter().map(String::as_str).collect()
        } else {
            result.pages.iter().map(|p| p.url.as_str()).collect()
        };
        let handle = fire_webhook(
            webhook_url,
            &serde_json::json!({
                "event": "crawl_complete",
                "total": result.total,
                "ok": result.ok,
                "errors": result.errors,
                "skipped_by_robots": result.skipped_by_robots,
                "elapsed_secs": result.elapsed_secs,
                "urls": urls,
            }),
        );
        let _ = handle.await;
    }

    if result.errors > 0 {
        Err(format!(
            "{} of {} pages failed",
            result.errors, result.total
        ))
    } else {
        Ok(())
    }
}

/// Web search via Serper.dev with the caller's own API key.
///
/// The Serper key is resolved by the caller (flag or `SERPER_API_KEY`
/// env, via clap's `env`) and passed in already-unwrapped. When `scrape`
/// is set, each result page is fetched + extracted through a FetchClient
/// (which carries the browser TLS profile) and its markdown is included.
#[allow(clippy::too_many_arguments)]
async fn run_search(
    serper_key: &str,
    query: &str,
    num: usize,
    country: Option<&str>,
    lang: Option<&str>,
    scrape: bool,
    format: &OutputFormat,
) -> Result<(), String> {
    // Default fetch config is enough: search localization is handled by
    // Serper's gl/hl, and the result-page scrape just needs a standard
    // browser profile. Attach cloud fallback when WEBCLAW_API_KEY is set
    // so scraped pages behind bot protection can still escalate.
    let mut client = webclaw_fetch::FetchClient::new(webclaw_fetch::FetchConfig::default())
        .map_err(|e| format!("client error: {e}"))?;
    if let Some(cloud) = webclaw_fetch::cloud::CloudClient::from_env() {
        client = client.with_cloud(cloud);
    }

    let opts = webclaw_fetch::SearchOptions {
        num_results: num,
        country: country.map(str::to_string),
        lang: lang.map(str::to_string),
        scrape,
    };

    let results = webclaw_fetch::search(&client, serper_key, query, &opts)
        .await
        .map_err(|e| format!("search error: {e}"))?;

    if matches!(format, OutputFormat::Json) {
        let json = serde_json::json!({ "query": query, "results": results });
        match serde_json::to_string_pretty(&json) {
            Ok(s) => println!("{s}"),
            Err(e) => return Err(format!("JSON encode failed: {e}")),
        }
        return Ok(());
    }

    if results.is_empty() {
        eprintln!("no results for \"{query}\"");
        return Ok(());
    }

    for r in &results {
        println!("{}. {}", r.position, r.title);
        println!("   {}", r.link);
        if !r.snippet.is_empty() {
            println!("   {}", r.snippet);
        }
        if let Some(ref content) = r.content {
            println!();
            println!("{content}");
        }
        println!();
    }

    Ok(())
}

async fn run_map(cli: &Cli) -> Result<(), String> {
    let url = cli
        .urls
        .first()
        .ok_or("--map requires a URL argument")
        .map(|u| normalize_url(u))?;
    let url = url.as_str();

    let mut opts = webclaw_fetch::MapOptions::default();
    opts.crawl_fallback = !cli.no_map_crawl;
    opts.max_urls = cli.map_limit;
    opts.html_only = !cli.map_all_captures;
    opts.path_prefix = cli.path_prefix.clone();
    opts.include_patterns = path_globs(cli.include_paths.as_deref());
    opts.exclude_patterns = path_globs(cli.exclude_paths.as_deref());
    opts.query = cli.query.clone();
    if let Some(pages) = cli.map_pages {
        opts.max_crawl_pages = pages;
    }
    if let Some(list) = &cli.map_source {
        opts.set_sources(list)?;
    }

    let mut config = build_fetch_config(cli);
    if opts.common_crawl {
        config.total_timeout = Some(std::time::Duration::from_secs(
            cli.timeout.saturating_mul(2),
        ));
    }
    let client = FetchClient::new(config).map_err(|e| format!("client error: {e}"))?;

    let site = webclaw_fetch::map_site(&client, url, &opts)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(e) = &site.index_error {
        let why = format!(
            "the Common Crawl lookup stopped early ({e}); the index is often busy, try again later"
        );
        if site.entries.is_empty() {
            return Err(why);
        }
        eprintln!("warning: {why}; listing the URLs found until then");
    }
    let entries = site.entries;

    if entries.is_empty() {
        eprintln!("no URLs found for {url}");
    } else {
        eprintln!("discovered {} URLs", entries.len());
    }

    print_map_output(&entries, &cli.format);
    Ok(())
}

async fn run_batch(cli: &Cli, entries: &[(String, Option<String>)]) -> Result<(), String> {
    let config = build_fetch_config(cli);
    let client =
        Arc::new(FetchClient::new(config.clone()).map_err(|e| format!("client error: {e}"))?);

    let urls: Vec<&str> = entries.iter().map(|(u, _)| u.as_str()).collect();
    let options = build_extraction_options(cli);
    let mut results = client
        .fetch_and_extract_batch_with_options(&urls, cli.concurrency, &options)
        .await;
    if cli.citations {
        cite_pages(
            results.iter_mut().filter_map(|r| r.result.as_mut().ok()),
            printed_together(cli),
        );
    }
    close_browser(&config).await;

    let ok = results.iter().filter(|r| r.result.is_ok()).count();
    let errors = results.len() - ok;

    for r in &results {
        if let Err(ref e) = r.result {
            eprintln!("error: {} -- {}", r.url, e);
        } else if let Ok(ref extraction) = r.result {
            let reason = detect_empty(extraction);
            if !matches!(reason, EmptyReason::None) {
                warn_empty(&r.url, &reason);
            }
        }
    }

    let custom_names: std::collections::HashMap<&str, &str> = entries
        .iter()
        .filter_map(|(url, name)| name.as_deref().map(|n| (url.as_str(), n)))
        .collect();

    if let Some(ref dir) = cli.output_dir {
        let mut saved = 0usize;
        for r in &results {
            if let Ok(ref extraction) = r.result {
                let filename = custom_names
                    .get(r.url.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| url_to_filename(&r.url, &cli.format));
                let content = format_output(extraction, &cli.format, cli.metadata);
                write_to_file(dir, &filename, &content)?;
                saved += 1;
            }
        }
        eprintln!("Saved {saved} files to {}", dir.display());
    } else if let Some(sizes) = chunk_sizes(cli) {
        let chunks: Vec<_> = results
            .iter()
            .filter_map(|r| {
                Some(chunk_page(
                    &r.result.as_ref().ok()?.content.markdown,
                    Some(&r.url),
                    sizes,
                ))
            })
            .flatten()
            .collect();
        print_chunks(&chunks, &cli.format);
    } else {
        print_batch_output(&results, &cli.format, cli.metadata);
    }

    eprintln!(
        "Fetched {} URLs ({} ok, {} errors)",
        results.len(),
        ok,
        errors
    );

    if let Some(ref webhook_url) = cli.webhook {
        let urls: Vec<&str> = results.iter().map(|r| r.url.as_str()).collect();
        let handle = fire_webhook(
            webhook_url,
            &serde_json::json!({
                "event": "batch_complete",
                "total": results.len(),
                "ok": ok,
                "errors": errors,
                "urls": urls,
            }),
        );
        let _ = handle.await;
    }

    if errors > 0 {
        Err(format!("{errors} of {} URLs failed", results.len()))
    } else {
        Ok(())
    }
}

fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hours = (now % 86400) / 3600;
    let minutes = (now % 3600) / 60;
    let seconds = now % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Spawn the `--on-change` command with `payload` on stdin.
///
/// Previously this passed the entire user-provided string to `sh -c`, which
/// made `--on-change 'notify "$URL"; rm -rf /'` a plausible disaster the
/// moment an untrusted config file or MCP-driven agent fed us a command.
/// The MCP surface specifically is prompt-injection-exposed: an LLM that
/// controls CLI args can escalate into arbitrary shell on the host.
///
/// We now parse the command with `shlex` (POSIX-ish tokenization with proper
/// quoting) and exec the program directly without an intermediate shell, so
/// metacharacters like `;`, `&&`, `|`, `$()`, and env expansion can't fire.
/// Users who genuinely need a pipeline can set the whole chain behind a
/// script they've written, or opt in per-call via `WEBCLAW_ALLOW_SHELL=1`
/// (documented escape hatch, noisy by design).
async fn spawn_on_change(cmd: &str, stdin_payload: &[u8]) {
    eprintln!("[watch] Running: {cmd}");

    let allow_shell = std::env::var("WEBCLAW_ALLOW_SHELL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let mut command = if allow_shell {
        eprintln!("[watch] WEBCLAW_ALLOW_SHELL=1 — executing via sh -c (unsafe)");
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    } else {
        let Some(argv) = shlex::split(cmd) else {
            eprintln!("[watch] Failed to parse --on-change command (unbalanced quotes?)");
            return;
        };
        let Some((program, args)) = argv.split_first() else {
            eprintln!("[watch] --on-change command is empty");
            return;
        };
        let mut c = tokio::process::Command::new(program);
        c.args(args);
        c
    };

    command.stdin(std::process::Stdio::piped());

    match command.spawn() {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(stdin_payload).await;
            }
        }
        Err(e) => eprintln!("[watch] Failed to run command: {e}"),
    }
}

/// Fire a webhook POST with a JSON payload. Spawns the send on a background task
/// and returns its `JoinHandle` so callers that need delivery (e.g. one-shot
/// crawl/batch runs that exit immediately after) can `.await` it; long-running
/// loops can drop the handle and let it run fire-and-forget. Errors are logged
/// to stderr. Auto-detects Discord and Slack webhook URLs and wraps the payload.
fn fire_webhook(url: &str, payload: &serde_json::Value) -> tokio::task::JoinHandle<()> {
    let url = url.to_string();
    let is_discord = url.contains("discord.com/api/webhooks");
    let is_slack = url.contains("hooks.slack.com");

    let body = if is_discord {
        let event = payload
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("notification");
        let details = serde_json::to_string_pretty(payload).unwrap_or_default();
        serde_json::json!({
            "embeds": [{
                "title": format!("webclaw: {event}"),
                "description": format!("```json\n{details}\n```"),
                "color": 5814783
            }]
        })
        .to_string()
    } else if is_slack {
        let event = payload
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("notification");
        let details = serde_json::to_string_pretty(payload).unwrap_or_default();
        serde_json::json!({
            "text": format!("*webclaw: {event}*\n```{details}```")
        })
        .to_string()
    } else {
        serde_json::to_string(payload).unwrap_or_default()
    };
    tokio::spawn(async move {
        // SSRF guard: a webhook URL is user-supplied and otherwise bypasses
        // the fetch-layer protections, so resolve + reject private/internal
        // destinations before sending the payload.
        if let Err(e) = webclaw_fetch::url_security::validate_public_http_url(&url).await {
            eprintln!("[webhook] refusing unsafe URL: {e}");
            return;
        }
        match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => match c
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .await
            {
                Ok(resp) => {
                    eprintln!(
                        "[webhook] POST {} -> {}",
                        &url[..url.len().min(60)],
                        resp.status()
                    );
                }
                Err(e) => eprintln!("[webhook] POST failed: {e}"),
            },
            Err(e) => eprintln!("[webhook] client error: {e}"),
        }
    })
}

async fn run_watch(cli: &Cli, urls: &[String]) -> Result<(), String> {
    if urls.is_empty() {
        return Err("--watch requires at least one URL".into());
    }

    let config = build_fetch_config(cli);
    let client =
        Arc::new(FetchClient::new(config.clone()).map_err(|e| format!("client error: {e}"))?);
    let options = build_extraction_options(cli);

    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        flag.store(true, Ordering::Relaxed);
    });

    let watched = if urls.len() == 1 {
        run_watch_single(cli, &client, &options, &urls[0], &cancelled).await
    } else {
        run_watch_multi(cli, &client, &options, urls, &cancelled).await
    };
    close_browser(&config).await;
    watched
}

/// Original single-URL watch loop -- backward compatible.
async fn run_watch_single(
    cli: &Cli,
    client: &Arc<FetchClient>,
    options: &ExtractionOptions,
    url: &str,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut previous = client
        .fetch_and_extract_with_options(url, options)
        .await
        .map_err(|e| format!("initial fetch failed: {e}"))?;

    eprintln!(
        "[watch] Initial snapshot: {url} ({} words)",
        previous.metadata.word_count
    );

    loop {
        // Clamp to >=1s: `--watch-interval 0` would otherwise spin the
        // fetch loop with zero delay and hammer the target.
        tokio::time::sleep(std::time::Duration::from_secs(cli.watch_interval.max(1))).await;

        if cancelled.load(Ordering::Relaxed) {
            eprintln!("[watch] Stopped");
            break;
        }

        let current = match client.fetch_and_extract_with_options(url, options).await {
            Ok(result) => result,
            Err(e) => {
                eprintln!("[watch] Fetch error ({}): {e}", timestamp());
                continue;
            }
        };

        let diff = webclaw_core::diff::diff(&previous, &current);

        if diff.status == ChangeStatus::Same {
            eprintln!("[watch] No changes ({})", timestamp());
        } else {
            print_diff_output(&diff, &cli.format);
            eprintln!("[watch] Changes detected! ({})", timestamp());

            if let Some(ref cmd) = cli.on_change {
                let diff_json = serde_json::to_string(&diff).unwrap_or_default();
                spawn_on_change(cmd, diff_json.as_bytes()).await;
            }

            if let Some(ref webhook_url) = cli.webhook {
                fire_webhook(
                    webhook_url,
                    &serde_json::json!({
                        "event": "watch_change",
                        "url": url,
                        "status": format!("{:?}", diff.status),
                        "word_count_delta": diff.word_count_delta,
                        "metadata_changes": diff.metadata_changes.len(),
                        "links_added": diff.links_added.len(),
                        "links_removed": diff.links_removed.len(),
                    }),
                );
            }

            previous = current;
        }
    }

    Ok(())
}

/// Multi-URL watch loop -- batch fetch all URLs, diff each, report aggregate.
async fn run_watch_multi(
    cli: &Cli,
    client: &Arc<FetchClient>,
    options: &ExtractionOptions,
    urls: &[String],
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let url_refs: Vec<&str> = urls.iter().map(|u| u.as_str()).collect();

    // Initial pass: fetch all URLs in parallel
    let initial_results = client
        .fetch_and_extract_batch_with_options(&url_refs, cli.concurrency, options)
        .await;

    let mut snapshots = std::collections::HashMap::new();
    let mut ok_count = 0usize;
    let mut err_count = 0usize;

    for r in initial_results {
        match r.result {
            Ok(extraction) => {
                snapshots.insert(r.url, extraction);
                ok_count += 1;
            }
            Err(e) => {
                eprintln!("[watch] Initial fetch error: {} -- {e}", r.url);
                err_count += 1;
            }
        }
    }

    eprintln!(
        "[watch] Watching {} URLs (interval: {}s)",
        urls.len(),
        cli.watch_interval
    );
    eprintln!("[watch] Initial snapshots: {ok_count} ok, {err_count} errors");

    let mut check_number = 0u64;

    loop {
        // Clamp to >=1s: `--watch-interval 0` would otherwise spin the
        // fetch loop with zero delay and hammer the target.
        tokio::time::sleep(std::time::Duration::from_secs(cli.watch_interval.max(1))).await;

        if cancelled.load(Ordering::Relaxed) {
            eprintln!("[watch] Stopped");
            break;
        }

        check_number += 1;

        let current_results = client
            .fetch_and_extract_batch_with_options(&url_refs, cli.concurrency, options)
            .await;

        let mut changed: Vec<serde_json::Value> = Vec::new();
        let mut same_count = 0usize;
        let mut fetch_errors = 0usize;

        for r in current_results {
            match r.result {
                Ok(current) => {
                    if let Some(previous) = snapshots.get(&r.url) {
                        let diff = webclaw_core::diff::diff(previous, &current);
                        if diff.status == ChangeStatus::Same {
                            same_count += 1;
                        } else {
                            changed.push(serde_json::json!({
                                "url": r.url,
                                "word_count_delta": diff.word_count_delta,
                            }));
                            snapshots.insert(r.url, current);
                        }
                    } else {
                        // URL failed initially, first successful fetch -- store as baseline
                        snapshots.insert(r.url, current);
                        same_count += 1;
                    }
                }
                Err(e) => {
                    eprintln!("[watch] Fetch error: {} -- {e}", r.url);
                    fetch_errors += 1;
                }
            }
        }

        let ts = timestamp();
        let err_suffix = if fetch_errors > 0 {
            format!(", {fetch_errors} errors")
        } else {
            String::new()
        };

        if changed.is_empty() {
            eprintln!(
                "[watch] Check {check_number} ({ts}): 0 changed, {same_count} same{err_suffix}"
            );
        } else {
            eprintln!(
                "[watch] Check {check_number} ({ts}): {} changed, {same_count} same{err_suffix}",
                changed.len(),
            );
            for entry in &changed {
                let url = entry["url"].as_str().unwrap_or("?");
                let delta = entry["word_count_delta"].as_i64().unwrap_or(0);
                eprintln!("  -> {url} (word delta: {delta:+})");
            }

            // Fire --on-change once with all changes
            if let Some(ref cmd) = cli.on_change {
                let payload = serde_json::json!({
                    "event": "watch_changes",
                    "check_number": check_number,
                    "total_urls": urls.len(),
                    "changed": changed.len(),
                    "same": same_count,
                    "changes": changed,
                });
                let payload_json = serde_json::to_string(&payload).unwrap_or_default();
                spawn_on_change(cmd, payload_json.as_bytes()).await;
            }

            // Fire webhook once with aggregate payload
            if let Some(ref webhook_url) = cli.webhook {
                fire_webhook(
                    webhook_url,
                    &serde_json::json!({
                        "event": "watch_changes",
                        "check_number": check_number,
                        "total_urls": urls.len(),
                        "changed": changed.len(),
                        "same": same_count,
                        "changes": changed,
                    }),
                );
            }
        }
    }

    Ok(())
}

async fn run_diff(cli: &Cli, snapshot_path: &str) -> Result<(), String> {
    // Load previous snapshot
    let snapshot_json = std::fs::read_to_string(snapshot_path)
        .map_err(|e| format!("failed to read snapshot {snapshot_path}: {e}"))?;
    let old: ExtractionResult = serde_json::from_str(&snapshot_json)
        .map_err(|e| format!("failed to parse snapshot JSON: {e}"))?;

    // Extract current version (handles PDF detection for URLs)
    let new_result = fetch_and_extract(cli).await?.into_extraction()?;

    let diff = webclaw_core::diff::diff(&old, &new_result);
    print_diff_output(&diff, &cli.format);

    Ok(())
}

async fn run_brand(cli: &Cli) -> Result<(), String> {
    let result = fetch_html(cli).await?;
    let enriched = enrich_html_with_stylesheets(&result.html, &result.url).await;
    let brand = webclaw_core::brand::extract_brand(
        &enriched,
        Some(result.url.as_str()).filter(|s| !s.is_empty()),
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&brand).expect("serialization failed")
    );
    Ok(())
}

/// Build an LLM provider based on CLI flags, or fall back to the default chain.
async fn build_llm_provider(cli: &Cli) -> Result<Box<dyn LlmProvider>, String> {
    if let Some(ref name) = cli.llm_provider {
        match name.as_str() {
            "ollama" => {
                let provider = webclaw_llm::providers::ollama::OllamaProvider::new(
                    cli.llm_base_url.clone(),
                    cli.llm_model.clone(),
                );
                if !provider.is_available().await {
                    return Err("ollama is not running or unreachable".into());
                }
                Ok(Box::new(provider))
            }
            "openai" => {
                let provider = webclaw_llm::providers::openai::OpenAiProvider::new(
                    None,
                    cli.llm_base_url.clone(),
                    cli.llm_model.clone(),
                )
                .ok_or("OPENAI_API_KEY not set")?;
                Ok(Box::new(provider))
            }
            "atlascloud" => {
                let provider = webclaw_llm::providers::atlascloud::AtlasCloudProvider::new(
                    None,
                    cli.llm_base_url.clone(),
                    cli.llm_model.clone(),
                )
                .ok_or("ATLASCLOUD_API_KEY not set")?;
                Ok(Box::new(provider))
            }
            "orcarouter" => {
                let provider = webclaw_llm::providers::orcarouter::OrcaRouterProvider::new(
                    None,
                    cli.llm_base_url.clone(),
                    cli.llm_model.clone(),
                )
                .ok_or("ORCAROUTER_API_KEY not set")?;
                Ok(Box::new(provider))
            }
            "anthropic" => {
                let provider = webclaw_llm::providers::anthropic::AnthropicProvider::with_base_url(
                    None,
                    cli.llm_base_url.clone(),
                    cli.llm_model.clone(),
                )
                .ok_or("ANTHROPIC_API_KEY not set")?;
                Ok(Box::new(provider))
            }
            other => Err(format!(
                "unknown LLM provider: {other} (use ollama, openai, atlascloud, anthropic, or orcarouter)"
            )),
        }
    } else {
        let chain = webclaw_llm::ProviderChain::default().await;
        if chain.is_empty() {
            return Err(
                "no LLM providers available -- start Ollama or set OPENAI_API_KEY / ANTHROPIC_API_KEY / ORCAROUTER_API_KEY"
                    .into(),
            );
        }
        Ok(Box::new(chain))
    }
}

async fn run_llm(cli: &Cli) -> Result<(), String> {
    // Extract content from source first (handles PDF detection for URLs)
    let result = fetch_and_extract(cli).await?.into_extraction()?;

    let provider = build_llm_provider(cli).await?;
    let model = cli.llm_model.as_deref();

    if let Some(ref schema_input) = cli.extract_json {
        // Support @file syntax for loading schema from file
        let schema_str = if let Some(path) = schema_input.strip_prefix('@') {
            std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read schema file {path}: {e}"))?
        } else {
            schema_input.clone()
        };

        let schema: serde_json::Value =
            serde_json::from_str(&schema_str).map_err(|e| format!("invalid JSON schema: {e}"))?;

        let extracted = webclaw_llm::extract::extract_json(
            &result.content.plain_text,
            &schema,
            provider.as_ref(),
            model,
        )
        .await
        .map_err(|e| format!("LLM extraction failed: {e}"))?;

        println!(
            "{}",
            serde_json::to_string_pretty(&extracted).expect("serialization failed")
        );
    } else if let Some(ref prompt) = cli.extract_prompt {
        let extracted = webclaw_llm::extract::extract_with_prompt(
            &result.content.plain_text,
            prompt,
            provider.as_ref(),
            model,
        )
        .await
        .map_err(|e| format!("LLM extraction failed: {e}"))?;

        println!(
            "{}",
            serde_json::to_string_pretty(&extracted).expect("serialization failed")
        );
    } else if let Some(sentences) = cli.summarize {
        let summary = webclaw_llm::summarize::summarize(
            &webclaw_core::to_llm_text(&result, None),
            Some(sentences),
            provider.as_ref(),
            model,
        )
        .await
        .map_err(|e| format!("LLM summarization failed: {e}"))?;

        println!("{summary}");
    }

    Ok(())
}

/// Batch LLM extraction: fetch each URL, run LLM on extracted content, save/print results.
/// URLs are processed sequentially to respect LLM provider rate limits.
async fn run_batch_llm(cli: &Cli, entries: &[(String, Option<String>)]) -> Result<(), String> {
    let config = build_fetch_config(cli);
    let client = FetchClient::new(config.clone()).map_err(|e| format!("client error: {e}"))?;
    let options = build_extraction_options(cli);
    let provider = build_llm_provider(cli).await?;
    let model = cli.llm_model.as_deref();

    // Pre-parse schema once if --extract-json is used
    let schema = if let Some(ref schema_input) = cli.extract_json {
        let schema_str = if let Some(path) = schema_input.strip_prefix('@') {
            std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read schema file {path}: {e}"))?
        } else {
            schema_input.clone()
        };
        Some(
            serde_json::from_str::<serde_json::Value>(&schema_str)
                .map_err(|e| format!("invalid JSON schema: {e}"))?,
        )
    } else {
        None
    };

    // Build custom filename lookup from entries
    let custom_names: std::collections::HashMap<&str, &str> = entries
        .iter()
        .filter_map(|(url, name)| name.as_deref().map(|n| (url.as_str(), n)))
        .collect();

    let total = entries.len();
    let mut ok = 0usize;
    let mut errors = 0usize;
    let mut all_results: Vec<serde_json::Value> = Vec::with_capacity(total);

    for (i, (url, _)) in entries.iter().enumerate() {
        let idx = i + 1;
        eprint!("[{idx}/{total}] {url} ");

        // Fetch and extract page content
        let extraction = match client.fetch_and_extract_with_options(url, &options).await {
            Ok(r) => r,
            Err(e) => {
                errors += 1;
                let msg = format!("fetch failed: {e}");
                eprintln!("-> error: {msg}");
                all_results.push(serde_json::json!({ "url": url, "error": msg }));
                continue;
            }
        };

        let text = &extraction.content.plain_text;

        // Run the appropriate LLM operation
        let llm_result = if let Some(ref schema) = schema {
            webclaw_llm::extract::extract_json(text, schema, provider.as_ref(), model)
                .await
                .map(LlmOutput::Json)
        } else if let Some(ref prompt) = cli.extract_prompt {
            webclaw_llm::extract::extract_with_prompt(text, prompt, provider.as_ref(), model)
                .await
                .map(LlmOutput::Json)
        } else if let Some(sentences) = cli.summarize {
            webclaw_llm::summarize::summarize(
                &webclaw_core::to_llm_text(&extraction, Some(url)),
                Some(sentences),
                provider.as_ref(),
                model,
            )
            .await
            .map(LlmOutput::Text)
        } else {
            unreachable!("run_batch_llm called without LLM flags")
        };

        match llm_result {
            Ok(output) => {
                ok += 1;

                let (output_str, result_json) = match &output {
                    LlmOutput::Json(v) => {
                        let s = serde_json::to_string_pretty(v).expect("serialization failed");
                        let j = serde_json::json!({ "url": url, "result": v });
                        (s, j)
                    }
                    LlmOutput::Text(s) => {
                        let j = serde_json::json!({ "url": url, "result": s });
                        (s.clone(), j)
                    }
                };

                // Count top-level fields/items for progress display
                let detail = match &output {
                    LlmOutput::Json(v) => match v {
                        serde_json::Value::Object(m) => format!("{} fields", m.len()),
                        serde_json::Value::Array(a) => format!("{} items", a.len()),
                        _ => "done".to_string(),
                    },
                    LlmOutput::Text(s) => {
                        let words = s.split_whitespace().count();
                        format!("{words} words")
                    }
                };
                eprintln!("-> extracted {detail}");

                if let Some(ref dir) = cli.output_dir {
                    let filename = custom_names
                        .get(url.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| url_to_filename(url, &OutputFormat::Json));
                    write_to_file(dir, &filename, &output_str)?;
                } else {
                    println!("--- {url}");
                    println!("{output_str}");
                    println!();
                }

                all_results.push(result_json);
            }
            Err(e) => {
                errors += 1;
                let msg = format!("LLM extraction failed: {e}");
                eprintln!("-> error: {msg}");
                all_results.push(serde_json::json!({ "url": url, "error": msg }));
            }
        }
    }

    close_browser(&config).await;
    eprintln!("Processed {total} URLs ({ok} ok, {errors} errors)");

    if let Some(ref webhook_url) = cli.webhook {
        let handle = fire_webhook(
            webhook_url,
            &serde_json::json!({
                "event": "batch_llm_complete",
                "total": total,
                "ok": ok,
                "errors": errors,
            }),
        );
        let _ = handle.await;
    }

    if errors > 0 {
        Err(format!("{errors} of {total} URLs failed"))
    } else {
        Ok(())
    }
}

/// Intermediate type to hold LLM output before formatting.
enum LlmOutput {
    Json(serde_json::Value),
    Text(String),
}

/// Returns true if any LLM flag is set.
fn has_llm_flags(cli: &Cli) -> bool {
    cli.extract_json.is_some() || cli.extract_prompt.is_some() || cli.summarize.is_some()
}

async fn run_research(cli: &Cli, query: &str) -> Result<(), String> {
    let api_key = cli
        .api_key
        .as_deref()
        .ok_or("--research requires WEBCLAW_API_KEY (set via env or --api-key)")?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| format!("http client error: {e}"))?;

    let mut body = serde_json::json!({ "query": query });
    if cli.deep {
        body["deep"] = serde_json::json!(true);
    }

    eprintln!("Starting research: {query}");
    if cli.deep {
        eprintln!("Deep mode enabled (longer, more thorough)");
    }

    // Start job
    let resp = client
        .post("https://api.webclaw.io/v1/research")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("API error: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse error: {e}"))?;

    let job_id = resp
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("API did not return a job ID")?
        .to_string();

    eprintln!("Job started: {job_id}");

    // Poll
    for poll in 0..200 {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let status_resp = client
            .get(format!("https://api.webclaw.io/v1/research/{job_id}"))
            .header("Authorization", format!("Bearer {api_key}"))
            .send()
            .await
            .map_err(|e| format!("poll error: {e}"))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("parse error: {e}"))?;

        let status = status_resp
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        match status {
            "completed" => {
                let report = status_resp
                    .get("report")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Save full result to JSON file
                let slug: String = query
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == ' ' {
                            c
                        } else {
                            ' '
                        }
                    })
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join("-")
                    .to_lowercase();
                // char-safe truncation: byte slicing panics if char 50
                // lands mid-codepoint (multibyte queries).
                let slug: String = slug.chars().take(50).collect();
                let filename = format!("research-{slug}.json");

                let json = serde_json::to_string_pretty(&status_resp).unwrap_or_default();
                std::fs::write(&filename, &json)
                    .map_err(|e| format!("failed to write {filename}: {e}"))?;

                let elapsed = status_resp
                    .get("elapsed_ms")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let sources = status_resp
                    .get("sources_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let findings = status_resp
                    .get("findings_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                eprintln!(
                    "Research complete: {sources} sources, {findings} findings, {:.1}s",
                    elapsed as f64 / 1000.0
                );
                eprintln!("Saved to: {filename}");

                // Print report to stdout
                if !report.is_empty() {
                    println!("{report}");
                }

                return Ok(());
            }
            "failed" => {
                let error = status_resp
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(format!("Research failed: {error}"));
            }
            _ => {
                if poll % 10 == 9 {
                    eprintln!("Still researching... ({:.0}s)", (poll + 1) as f64 * 3.0);
                }
            }
        }
    }

    Err(format!(
        "Research timed out after ~10 minutes. Check status: GET /v1/research/{job_id}"
    ))
}

fn exit(code: i32) -> ! {
    #[cfg(feature = "browser")]
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| runtime.block_on(webclaw_fetch::render::close_profiles()));
    }
    process::exit(code)
}

#[tokio::main]
async fn main() {
    run().await;
    #[cfg(feature = "browser")]
    webclaw_fetch::render::close_profiles().await;
}

async fn run() {
    dotenvy::dotenv().ok();

    let mut cli = Cli::parse();
    init_logging(cli.verbose);
    if let Err(e) = check_warc(&cli) {
        eprintln!("error: {e}");
        exit(1);
    }

    if cli.browser_profile.is_some() && (!BROWSER_BUILD || cli.render == RenderMode::Never) {
        eprintln!(
            "error: --browser-profile needs the browser: a build with the `browser` feature, and --render auto or always"
        );
        exit(1);
    }
    #[cfg(all(unix, feature = "browser"))]
    webclaw_fetch::render::close_profiles_on_terminate();
    #[cfg(feature = "browser")]
    if cli.browser_profile.is_some()
        && let Some(render) = build_fetch_config(&cli).render
        && let Err(e) = render.pool.open().await
    {
        eprintln!("error: {e}");
        exit(1);
    }
    if cli.solve && (!BROWSER_BUILD || cli.render == RenderMode::Never) {
        eprintln!(
            "error: --solve needs the browser: a build with `--features browser`, and no --render never"
        );
        exit(1);
    }

    if let Some(ref cmd) = cli.command {
        match cmd {
            Commands::Bench { url, json, facts } => {
                let args = bench::BenchArgs {
                    url: url.clone(),
                    json: *json,
                    facts: facts.clone(),
                };
                if let Err(e) = bench::run(&args).await {
                    eprintln!("error: {e}");
                    exit(1);
                }
                return;
            }
            Commands::Extractors { json } => {
                let entries = webclaw_fetch::extractors::list();
                if *json {
                    match serde_json::to_string_pretty(&entries) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("error: failed to serialise catalog: {e}");
                            exit(1);
                        }
                    }
                } else {
                    println!("{} vertical extractors available:\n", entries.len());
                    let name_w = entries.iter().map(|e| e.name.len()).max().unwrap_or(0);
                    let label_w = entries.iter().map(|e| e.label.len()).max().unwrap_or(0);
                    for e in &entries {
                        let pattern_sample = e.url_patterns.first().copied().unwrap_or("");
                        println!(
                            "  {:<nw$}  {:<lw$}  {}",
                            e.name,
                            e.label,
                            pattern_sample,
                            nw = name_w,
                            lw = label_w,
                        );
                    }
                    println!("\nRun one: webclaw vertical <name> <url>");
                }
                return;
            }
            Commands::Vertical { name, url, raw } => {
                let fetch_cfg = webclaw_fetch::FetchConfig {
                    browser: webclaw_fetch::BrowserProfile::Firefox,
                    ..webclaw_fetch::FetchConfig::default()
                };
                let mut client = match webclaw_fetch::FetchClient::new(fetch_cfg) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("error: failed to build fetch client: {e}");
                        exit(1);
                    }
                };
                if let Some(cloud) = webclaw_fetch::cloud::CloudClient::from_env() {
                    client = client.with_cloud(cloud);
                }
                match webclaw_fetch::extractors::dispatch_by_name(&client, name, url).await {
                    Ok(data) => {
                        let rendered = if *raw {
                            serde_json::to_string(&data)
                        } else {
                            serde_json::to_string_pretty(&data)
                        };
                        match rendered {
                            Ok(s) => println!("{s}"),
                            Err(e) => {
                                eprintln!("error: JSON encode failed: {e}");
                                exit(1);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        exit(1);
                    }
                }
                return;
            }
            Commands::Search {
                query,
                serper_key,
                num,
                country,
                lang,
                scrape,
                format,
            } => {
                let key = match serper_key {
                    Some(k) if !k.trim().is_empty() => k.clone(),
                    _ => {
                        eprintln!(
                            "error: search requires a Serper.dev API key: pass --serper-key or set SERPER_API_KEY (get one free at serper.dev)"
                        );
                        exit(1);
                    }
                };
                if let Err(e) = run_search(
                    &key,
                    query,
                    *num,
                    country.as_deref(),
                    lang.as_deref(),
                    *scrape,
                    format,
                )
                .await
                {
                    eprintln!("error: {e}");
                    exit(1);
                }
                return;
            }
        }
    }

    if cli.map {
        if let Err(e) = run_map(&cli).await {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    let entries = match collect_urls(&cli) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: {e}");
            exit(1);
        }
    };
    backfill_single_file_url(&mut cli.urls, &entries);
    if let Err(e) = check_markdown_flags(&cli) {
        eprintln!("error: {e}");
        exit(1);
    }

    if cli.actions.is_some() || cli.scroll.is_some() {
        if entries.len() != 1 {
            eprintln!("error: {}", one_url_error(&cli));
            exit(1);
        }
        if cli.render == RenderMode::Never {
            eprintln!(
                "error: --actions and --scroll run in the browser, so they cannot run with --render never"
            );
            exit(1);
        }
        cli.render = RenderMode::Always;
    }

    if let Some(ref path) = cli.screenshot {
        let shot = match (entries.len(), cli.urls.first()) {
            (1, Some(url)) => run_screenshot(&cli, &normalize_url(url), path).await,
            _ => Err("--screenshot takes one URL".to_string()),
        };
        if let Err(e) = shot {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if let Some(ref pattern) = cli.capture_network {
        let captured = match (entries.len(), cli.urls.first()) {
            (1, Some(url)) => run_capture(&cli, &normalize_url(url), pattern).await,
            _ => Err(
                "--capture-network takes one URL; give a pattern as --capture-network=PATTERN"
                    .to_string(),
            ),
        };
        if let Err(e) = captured {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    let save = (cli.save_pdf.as_ref().map(|path| (path, true)))
        .or(cli.save_mhtml.as_ref().map(|path| (path, false)));
    if let Some((path, pdf)) = save {
        let printed = match (entries.len(), cli.urls.first()) {
            (1, Some(url)) => run_save(&cli, &normalize_url(url), path, pdf).await,
            _ if pdf => Err("--save-pdf takes one URL".to_string()),
            _ => Err("--save-mhtml takes one URL".to_string()),
        };
        if let Err(e) = printed {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if cli.tables {
        let found = if entries.len() > 1 {
            Err("--tables takes one URL".to_string())
        } else {
            run_tables(&cli).await
        };
        if let Err(e) = found {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if cli.select.is_some()
        || cli.xpath.is_some()
        || cli.find_text.is_some()
        || cli.find_regex.is_some()
    {
        let selected = if entries.len() > 1 {
            Err("--select, --xpath, --find-text and --find-regex take one URL".to_string())
        } else {
            run_select(&cli).await
        };
        if let Err(e) = selected {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if cli.crawl {
        if let Err(e) = run_crawl(&cli).await {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if cli.watch {
        let watch_urls: Vec<String> = entries.iter().map(|(url, _)| url.clone()).collect();
        if let Err(e) = run_watch(&cli, &watch_urls).await {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if let Some(ref snapshot_path) = cli.diff_with {
        if let Err(e) = run_diff(&cli, snapshot_path).await {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if cli.brand {
        if let Err(e) = run_brand(&cli).await {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if let Some(ref query) = cli.research {
        if let Err(e) = run_research(&cli, query).await {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if has_llm_flags(&cli) {
        if entries.len() > 1 {
            if let Err(e) = run_batch_llm(&cli, &entries).await {
                eprintln!("error: {e}");
                exit(1);
            }
        } else if let Err(e) = run_llm(&cli).await {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if cli.query.is_some() && entries.len() > 1 {
        eprintln!("error: --query works on one page at a time");
        exit(1);
    }

    if entries.len() > 1 {
        if let Err(e) = run_batch(&cli, &entries).await {
            eprintln!("error: {e}");
            exit(1);
        }
        return;
    }

    if cli.raw_html && cli.include.is_none() && cli.exclude.is_none() {
        let page = if cli.actions.is_some() || cli.scroll.is_some() {
            fetch_page(&cli).await
        } else {
            fetch_html(&cli).await
        };
        match page {
            Ok(r) => {
                println!("{}", r.html);
                if let Err(e) = webclaw_fetch::FetchError::check_status(r.status) {
                    eprintln!("error: {e}");
                    exit(1);
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                exit(1);
            }
        }
        return;
    }

    match fetch_and_extract(&cli).await {
        Ok(FetchOutput::Local(mut result, js_results)) => {
            if let Some(ref query) = cli.query {
                webclaw_core::focus_extraction(&mut result, query);
            }
            if cli.citations {
                cite_pages([&mut *result], false);
            }
            if let Some(ref dir) = cli.output_dir {
                let url = cli
                    .urls
                    .first()
                    .map(|u| normalize_url(u))
                    .unwrap_or_default();
                let custom_name = entries.first().and_then(|(_, name)| name.clone());
                let filename = custom_name.unwrap_or_else(|| url_to_filename(&url, &cli.format));
                let content = format_output(&result, &cli.format, cli.metadata);
                if let Err(e) = write_to_file(dir, &filename, &content) {
                    eprintln!("error: {e}");
                    exit(1);
                }
            } else if let Some(sizes) = chunk_sizes(&cli) {
                let chunks = chunk_page(
                    &result.content.markdown,
                    result.metadata.url.as_deref(),
                    sizes,
                );
                print_chunks(&chunks, &cli.format);
            } else if !js_results.is_empty()
                && matches!(cli.format, OutputFormat::Json | OutputFormat::Jsonl)
            {
                println!(
                    "{}",
                    with_js_results(
                        &result,
                        &js_results,
                        matches!(cli.format, OutputFormat::Json)
                    )
                );
            } else {
                print_output(&result, &cli.format, cli.metadata);
            }
            if !matches!(cli.format, OutputFormat::Json | OutputFormat::Jsonl)
                || cli.output_dir.is_some()
                || chunk_sizes(&cli).is_some()
            {
                print_script_results(&js_results);
            }
        }
        Ok(FetchOutput::Cloud(mut resp)) => {
            if let Some(ref query) = cli.query {
                webclaw_fetch::cloud::focus_cloud_response(&mut resp, query);
            }
            if cli.citations {
                let url = cli.urls.first().map(|u| normalize_url(u));
                webclaw_fetch::cloud::cite_cloud_response(&mut resp, url.as_deref());
            }
            if let Some(sizes) = chunk_sizes(&cli) {
                let url = cli.urls.first().map(|u| normalize_url(u));
                let chunks = chunk_page(
                    cloud_text(&resp, "markdown", "markdown").unwrap_or_default(),
                    url.as_deref(),
                    sizes,
                );
                print_chunks(&chunks, &cli.format);
            } else {
                print_cloud_output(&resp, &cli.format);
            }
        }
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use webclaw_core::Content;

    #[test]
    fn a_map_limit_of_zero_is_refused() {
        assert!(
            Cli::try_parse_from(["webclaw", "--map", "--map-limit", "0", "https://a.com"]).is_err()
        );
        let cli =
            Cli::try_parse_from(["webclaw", "--map", "--map-limit", "1", "https://a.com"]).unwrap();
        assert_eq!(cli.map_limit, Some(1));
    }

    #[test]
    fn scroll_takes_its_count_only_after_an_equals_sign() {
        let cli = Cli::try_parse_from(["webclaw", "--scroll", "https://example.com"]).unwrap();
        assert_eq!(cli.scroll, Some(20));
        assert_eq!(cli.urls, vec!["https://example.com".to_string()]);
        let cli = Cli::try_parse_from(["webclaw", "--scroll=5", "https://example.com"]).unwrap();
        assert_eq!(cli.scroll, Some(5));
    }

    #[test]
    fn json_output_carries_js_results_and_raw_html_takes_actions() {
        let result = empty_result(Some("t"), Some("https://example.com/"), "body");
        let json: serde_json::Value = serde_json::from_str(&with_js_results(
            &result,
            &[serde_json::json!(3), serde_json::json!({"a": 1})],
            true,
        ))
        .unwrap();
        assert_eq!(json["js_results"], serde_json::json!([3, {"a": 1}]));
        assert_eq!(json["content"]["markdown"], "body");
        assert!(
            Cli::try_parse_from([
                "webclaw",
                "https://example.com",
                "--actions",
                "[]",
                "--raw-html"
            ])
            .is_ok()
        );
    }

    #[test]
    fn a_scroll_count_after_a_space_gets_a_hint() {
        let cli = Cli::try_parse_from(["webclaw", "--scroll", "3", "https://example.com"]).unwrap();
        assert_eq!(
            one_url_error(&cli),
            "--actions and --scroll take one URL; for a scroll count write --scroll=3"
        );
        let cli =
            Cli::try_parse_from(["webclaw", "--scroll", "https://a.com", "https://b.com"]).unwrap();
        assert_eq!(one_url_error(&cli), "--actions and --scroll take one URL");
    }

    #[test]
    fn one_picker_at_a_time_and_tables_take_no_pick_options() {
        let parse = |extra: &[&str]| {
            let mut args = vec!["webclaw", "https://example.com"];
            args.extend_from_slice(extra);
            Cli::try_parse_from(args)
        };
        assert!(parse(&["--tables"]).is_ok());
        assert!(parse(&["--find-text", "a", "--exact", "--similar"]).is_ok());
        assert!(parse(&["--select", "a", "--similar", "--field", "x=b"]).is_ok());
        for extra in [
            &["--tables", "--select", "a"][..],
            &["--tables", "--xpath", "//a"],
            &["--tables", "--find-text", "a"],
            &["--tables", "--find-regex", "a"],
            &["--tables", "--similar"],
            &["--tables", "--field", "x=b"],
            &["--tables", "--exact"],
            &["--tables", "--case-sensitive"],
            &["--select", "a", "--find-text", "b"],
        ] {
            assert!(parse(extra).is_err(), "{extra:?}");
        }
    }

    #[test]
    fn chunk_flags_parse_and_refuse_modes_they_cannot_join() {
        let cli = Cli::try_parse_from([
            "webclaw",
            "https://example.com",
            "--chunk",
            "256",
            "--chunk-overlap",
            "32",
        ])
        .unwrap();
        assert_eq!(chunk_sizes(&cli), Some((256, 32)));
        for args in [
            vec!["webclaw", "https://example.com", "--chunk", "0"],
            vec!["webclaw", "https://example.com", "--chunk-overlap", "8"],
            vec![
                "webclaw",
                "https://example.com",
                "--chunk",
                "64",
                "--select",
                "a",
            ],
            vec![
                "webclaw",
                "https://example.com",
                "--chunk",
                "64",
                "--raw-html",
            ],
            vec![
                "webclaw",
                "https://example.com",
                "--chunk",
                "64",
                "--find-text",
                "a",
            ],
        ] {
            assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
        }
    }

    #[cfg(feature = "browser")]
    #[tokio::test]
    async fn a_failed_save_pdf_leaves_an_existing_file_alone() {
        let dir = std::env::temp_dir().join(format!("webclaw-save-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("page.pdf");
        std::fs::write(&path, "earlier PDF").unwrap();
        let cli = Cli::try_parse_from([
            "webclaw",
            "https://example.com",
            "--cdp-url",
            "ws://127.0.0.1:9/no-browser",
            "--save-pdf",
            path.to_str().unwrap(),
        ])
        .unwrap();
        let saved = run_save(&cli, "https://example.com/", &path, true).await;
        let kept = std::fs::read_to_string(&path).unwrap();
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(saved.is_err());
        assert_eq!(kept, "earlier PDF");
        assert_eq!(leftovers, 1);
    }

    #[test]
    fn a_new_run_appends_to_an_existing_warc_with_its_own_warcinfo() {
        let dir = std::env::temp_dir().join(format!("webclaw-warc-append-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("site.warc");
        drop(open_warc(&path).unwrap());
        let earlier = std::fs::read(&path).unwrap();
        drop(open_warc(&path).unwrap());
        let file = std::fs::read(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(file.starts_with(&earlier));
        let text = String::from_utf8_lossy(&file);
        assert_eq!(text.matches("WARC-Type: warcinfo").count(), 2);
    }

    #[test]
    fn a_warc_cut_inside_a_record_is_refused_and_left_alone() {
        let dir = std::env::temp_dir().join(format!("webclaw-warc-cut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["site.warc", "site.warc.gz"] {
            let path = dir.join(name);
            drop(open_warc(&path).unwrap());
            let whole = std::fs::read(&path).unwrap();
            std::fs::write(&path, &whole[..whole.len() - 5]).unwrap();
            let refused = open_warc(&path).err().map(|e| e.to_string());
            assert!(
                refused.as_deref().is_some_and(|e| e.contains("new file")),
                "{name}: {refused:?}"
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                whole[..whole.len() - 5],
                "{name}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn warc_refuses_what_never_goes_over_http() {
        let check = |args: &[&str]| check_warc(&Cli::try_parse_from(args).unwrap());
        assert!(check(&["webclaw", "https://example.com", "--warc", "a.warc"]).is_ok());
        assert!(
            check(&[
                "webclaw",
                "https://example.com",
                "--warc",
                "a.warc",
                "--render",
                "always"
            ])
            .is_err()
        );
        assert!(check(&["webclaw", "--warc", "a.warc", "extractors"]).is_err());
        assert!(check(&["webclaw", "https://example.com", "--warc", "-"]).is_err());
        assert!(
            check(&[
                "webclaw",
                "--warc",
                "a.warc",
                "vertical",
                "github_repo",
                "https://github.com/a/b"
            ])
            .is_err()
        );
    }

    #[test]
    fn warc_goes_with_fetches_and_crawls_but_not_with_other_sources() {
        let parse = |extra: &[&str]| {
            let mut args = vec!["webclaw", "https://example.com", "--warc", "a.warc.gz"];
            args.extend_from_slice(extra);
            Cli::try_parse_from(args)
        };
        for fine in [
            &[][..],
            &["--crawl", "--depth", "2"],
            &["-f", "json"],
            &["--raw-html"],
        ] {
            assert!(parse(fine).is_ok(), "{fine:?}");
        }
        for refused in [
            &["--cache"][..],
            &["--cloud"],
            &["--stdin"],
            &["--file", "page.html"],
            &["--screenshot", "a.png"],
            &["--save-pdf", "a.pdf"],
            &["--save-mhtml", "a.mhtml"],
            &["--capture-network"],
            &["--full-page"],
            &["--paper", "a4"],
            &["--landscape"],
            &["--print-background"],
            &["--scale", "0.5"],
            &["--capture-types", "xhr"],
            &["--capture-credentials"],
        ] {
            assert!(parse(refused).is_err(), "{refused:?}");
        }
        for refused in [
            &["--full-page"][..],
            &["--capture-types", "xhr"],
            &["--capture-credentials"],
        ] {
            let mut args = vec!["webclaw", "https://example.com", "--save-mhtml", "a.mhtml"];
            args.extend_from_slice(refused);
            assert!(Cli::try_parse_from(args).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn pdf_flags_need_save_pdf_and_refuse_other_outputs() {
        let parse = |extra: &[&str]| {
            let mut args = vec!["webclaw", "https://example.com"];
            args.extend_from_slice(extra);
            Cli::try_parse_from(args)
        };
        let cli = parse(&[
            "--save-pdf",
            "page.pdf",
            "--paper",
            "a4",
            "--landscape",
            "--print-background",
            "--scale",
            "0.8",
            "--wait-for",
            "main",
        ])
        .unwrap();
        assert!(matches!(cli.paper, PaperArg::A4));
        assert_eq!(cli.scale, 0.8);
        for extra in [
            &["--landscape"][..],
            &["--paper", "a4"],
            &["--print-background"],
            &["--scale", "2"],
            &["--save-pdf", "a.pdf", "--paper", "a3"],
            &["--save-pdf", "a.pdf", "--scale", "3"],
            &["--save-pdf", "a.pdf", "--scale", "0"],
            &["--save-pdf", "a.pdf", "--screenshot", "a.png"],
            &["--save-pdf", "a.pdf", "--select", "a"],
            &["--save-pdf", "a.pdf", "--crawl"],
            &["--save-pdf", "a.pdf", "--chunk", "64"],
            &["--save-pdf", "a.pdf", "--block", "images"],
        ] {
            assert!(parse(extra).is_err(), "{extra:?}");
        }
        assert!(parse(&["--screenshot", "a.png"]).is_ok());
        assert!(parse(&["--save-mhtml", "a.mhtml", "--wait-for", "main", "--scroll"]).is_ok());
        for extra in [
            &["--save-mhtml", "a.mhtml", "--save-pdf", "a.pdf"][..],
            &["--save-mhtml", "a.mhtml", "--screenshot", "a.png"],
            &["--save-mhtml", "a.mhtml", "--print-background"],
            &["--save-mhtml", "a.mhtml", "--crawl"],
            &["--save-mhtml", "a.mhtml", "--block", "images"],
            &["--save-mhtml", "a.mhtml", "--solve"],
            &["--save-mhtml", "a.mhtml", "--chunk", "64"],
            &["--save-mhtml", "a.mhtml", "--capture-network"],
            &["--save-mhtml", "a.mhtml", "--warc", "a.warc"],
        ] {
            assert!(parse(extra).is_err(), "{extra:?}");
        }
        for output in ["--screenshot", "--save-pdf", "--save-mhtml"] {
            for other in [
                &["-f", "json"][..],
                &["--summarize"],
                &["--cloud"],
                &["--output-dir", "out"],
                &["--diff-with", "old.json"],
                &["--brand"],
                &["--raw-html"],
                &["--pdf-mode", "fast"],
                &["--metadata"],
                &["--query", "price"],
                &["--extract-prompt", "title"],
                &["--include", "main"],
                &["--exclude", "nav"],
                &["--only-main-content"],
                &["--stdin"],
                &["--file", "page.html"],
            ] {
                let mut extra = vec![output, "a.out"];
                extra.extend_from_slice(other);
                assert!(parse(&extra).is_err(), "{extra:?}");
            }
        }
    }

    #[test]
    fn cloud_format_preserves_extraction_for_diff_and_llm_modes() {
        for args in [
            vec![
                "webclaw",
                "https://example.com",
                "--diff-with",
                "before.json",
            ],
            vec!["webclaw", "https://example.com", "--summarize", "2"],
            vec!["webclaw", "https://example.com", "--extract-json", "{}"],
            vec![
                "webclaw",
                "https://example.com",
                "--extract-prompt",
                "Title",
            ],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(cloud_scrape_format(&cli), "json");
        }
        let cli =
            Cli::try_parse_from(["webclaw", "https://example.com", "--format", "text"]).unwrap();
        assert_eq!(cloud_scrape_format(&cli), "text");
    }

    // issue #86: a single URL sourced only from --urls-file must be promoted to
    // a positional URL so it takes the single-scrape path (batch gates need >1).
    #[test]
    fn cloud_renderer_uses_requested_top_level_formats() {
        let response = serde_json::json!({"markdown": "**Markdown**", "text": "Plain text", "llm": "LLM text", "rawHtml": "<p>HTML</p>", "content": {"plain_text": "Old text"}});
        assert_eq!(
            cloud_text(&response, "text", "plain_text"),
            Some("Plain text")
        );
        assert_eq!(cloud_text(&response, "llm", "llm_text"), Some("LLM text"));
        assert_eq!(
            cloud_text(&response, "rawHtml", "raw_html"),
            Some("<p>HTML</p>")
        );
        let legacy = serde_json::json!({"content": {"plain_text": "Old text"}});
        assert_eq!(cloud_text(&legacy, "text", "plain_text"), Some("Old text"));
    }

    #[test]
    fn single_file_url_backfilled_into_positional() {
        let mut urls: Vec<String> = Vec::new();
        let entries = vec![("https://example.com".to_string(), None)];
        backfill_single_file_url(&mut urls, &entries);
        assert_eq!(urls, vec!["https://example.com".to_string()]);
    }

    #[test]
    fn positional_urls_left_untouched() {
        let mut urls = vec!["https://a.com".to_string()];
        let entries = vec![("https://a.com".to_string(), None)];
        backfill_single_file_url(&mut urls, &entries);
        assert_eq!(urls, vec!["https://a.com".to_string()]);
    }

    #[test]
    fn multiple_file_urls_not_backfilled() {
        // 2+ URLs already route to run_batch, so cli.urls stays empty here.
        let mut urls: Vec<String> = Vec::new();
        let entries = vec![
            ("https://a.com".to_string(), None),
            ("https://b.com".to_string(), None),
        ];
        backfill_single_file_url(&mut urls, &entries);
        assert!(urls.is_empty());
    }

    #[test]
    fn citations_refuse_modes_that_print_no_page_markdown() {
        for mode in [
            &["--select", "a"][..],
            &["--capture-network", ""],
            &["--xpath", "//a"],
            &["--find-text", "x"],
            &["--tables"],
            &["--map"],
            &["--brand"],
            &["--diff-with", "old.json"],
            &["--watch"],
            &["--summarize"],
            &["--extract-prompt", "x"],
            &["--chunk", "100"],
            &["--raw-html"],
        ] {
            let args = ["webclaw", "https://example.com", "--citations"]
                .into_iter()
                .chain(mode.iter().copied());
            assert!(Cli::try_parse_from(args).is_err(), "{mode:?}");
        }
        assert!(
            Cli::try_parse_from([
                "webclaw",
                "https://example.com",
                "--citations",
                "--metadata"
            ])
            .is_ok()
        );
    }

    #[test]
    fn citations_continue_across_pages_printed_together() {
        let pages = || {
            vec![
                empty_result(None, None, "[a](https://a.com)"),
                empty_result(None, None, "[b](https://b.com)"),
            ]
        };
        let mut together = pages();
        cite_pages(together.iter_mut(), true);
        assert!(together[1].content.markdown.starts_with("b[^2]"));
        let mut apart = pages();
        cite_pages(apart.iter_mut(), false);
        assert!(apart[1].content.markdown.starts_with("b[^1]"));
    }

    fn empty_result(title: Option<&str>, url: Option<&str>, markdown: &str) -> ExtractionResult {
        ExtractionResult {
            metadata: Metadata {
                title: title.map(str::to_string),
                description: None,
                author: None,
                published_date: None,
                language: None,
                url: url.map(str::to_string),
                site_name: None,
                image: None,
                favicon: None,
                word_count: markdown.split_whitespace().count(),
            },
            content: Content {
                markdown: markdown.to_string(),
                plain_text: markdown.to_string(),
                links: vec![],
                images: vec![],
                code_blocks: vec![],
                raw_html: None,
            },
            domain_data: None,
            structured_data: vec![],
        }
    }

    #[test]
    fn detect_empty_identifies_consent_redirect_url() {
        let result = empty_result(
            Some("Yahoo"),
            Some("https://guce.advertising.com/collectIdentifiers?sessionId=abc"),
            "Continue",
        );
        assert_eq!(detect_empty(&result), EmptyReason::ConsentWall);
    }

    #[test]
    fn detect_empty_identifies_short_consent_title() {
        let result = empty_result(
            Some("Before you continue"),
            Some("https://www.google.com/"),
            "Review privacy options",
        );
        assert_eq!(detect_empty(&result), EmptyReason::ConsentWall);
    }

    #[test]
    fn detect_empty_does_not_flag_real_content_with_consent_words() {
        let result = empty_result(
            Some("Cookie consent patterns explained"),
            Some("https://example.com/blog"),
            "This article explains cookie consent patterns for product teams with enough real body text to be useful. It covers consent banners, privacy controls, analytics configuration, regional requirements, product tradeoffs, implementation details, testing flows, debugging notes, accessibility needs, and operational lessons from real teams shipping public websites across multiple markets. It also explains measurement, rollout planning, copy review, support workflows, design constraints, release notes, and how to keep privacy choices understandable for users.",
        );
        assert_eq!(detect_empty(&result), EmptyReason::None);
    }

    #[test]
    fn url_to_filename_root() {
        assert_eq!(
            url_to_filename("https://example.com/", &OutputFormat::Markdown),
            "example_com/index.md"
        );
        assert_eq!(
            url_to_filename("https://example.com", &OutputFormat::Markdown),
            "example_com/index.md"
        );
    }

    #[test]
    fn url_to_filename_path() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Markdown),
            "docs/api.md"
        );
    }

    #[test]
    fn url_to_filename_trailing_slash() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api/", &OutputFormat::Markdown),
            "docs/api.md"
        );
    }

    #[test]
    fn url_to_filename_nested_path() {
        assert_eq!(
            url_to_filename("https://example.com/blog/my-post", &OutputFormat::Markdown),
            "blog/my-post.md"
        );
    }

    #[test]
    fn url_to_filename_query_params() {
        assert_eq!(
            url_to_filename("https://example.com/p?id=123", &OutputFormat::Markdown),
            "p_id_123.md"
        );
    }

    #[test]
    fn url_to_filename_json_format() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Json),
            "docs/api.json"
        );
    }

    #[test]
    fn url_to_filename_text_format() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Text),
            "docs/api.txt"
        );
    }

    #[test]
    fn url_to_filename_llm_format() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Llm),
            "docs/api.md"
        );
    }

    #[test]
    fn url_to_filename_html_format() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Html),
            "docs/api.html"
        );
    }

    #[test]
    fn url_to_filename_special_chars() {
        // Spaces and special chars get replaced with underscores
        assert_eq!(
            url_to_filename(
                "https://example.com/path%20with%20spaces",
                &OutputFormat::Markdown
            ),
            "path_20with_20spaces.md"
        );
    }

    #[test]
    fn write_to_file_creates_dirs() {
        let dir = std::env::temp_dir().join("webclaw_test_output_dir");
        let _ = std::fs::remove_dir_all(&dir);
        write_to_file(&dir, "nested/deep/file.md", "hello").unwrap();
        let content = std::fs::read_to_string(dir.join("nested/deep/file.md")).unwrap();
        assert_eq!(content, "hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_to_filename_strips_traversal_segments() {
        // `..` / `.` / empty path segments must not survive into the path.
        let out = url_to_filename(
            "https://example.com/../../etc/passwd",
            &OutputFormat::Markdown,
        );
        assert!(!out.contains(".."), "traversal leaked: {out}");
        assert_eq!(out, "etc/passwd.md");
        let out2 = url_to_filename("https://example.com/a/./b//c", &OutputFormat::Json);
        assert_eq!(out2, "a/b/c.json");
    }

    #[test]
    fn safe_relative_filename_rejects_escapes() {
        assert!(safe_relative_filename("../escape.md").is_err());
        assert!(safe_relative_filename("a/../../b.md").is_err());
        assert!(safe_relative_filename("/etc/passwd").is_err());
        assert!(safe_relative_filename("").is_err());
        // Normal nested relative names stay allowed.
        assert!(safe_relative_filename("nested/deep/file.md").is_ok());
        assert!(safe_relative_filename("./ok.md").is_ok());
    }

    #[test]
    fn write_to_file_refuses_traversal_filename() {
        let dir = std::env::temp_dir().join("webclaw_test_traversal_dir");
        let _ = std::fs::remove_dir_all(&dir);
        // CSV-supplied `url,filename` traversal attempt.
        let err = write_to_file(&dir, "../../tmp/webclaw_pwned.md", "x").unwrap_err();
        assert!(err.contains("refusing"), "unexpected error: {err}");
        assert!(
            !std::path::Path::new("/tmp/webclaw_pwned.md").exists(),
            "traversal write escaped the output dir"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // M14: URL truncation heuristic tests.
    #[test]
    fn looks_truncated_fires_on_trailing_ampersand() {
        // The most common shell-split shape: `?a=1&` lost the `b=2`.
        assert!(looks_truncated("https://example.com/?a=1&"));
        assert!(looks_truncated("https://example.com/path?key=val&"));
    }

    #[test]
    fn looks_truncated_fires_on_query_with_no_equals() {
        // `?foo` with no `=` is a strong signal the shell ate the `=value`.
        assert!(looks_truncated("https://example.com/?foo"));
        // Bare `?` (empty query) also looks like the shell ate the whole pair.
        assert!(looks_truncated("https://example.com/?"));
        // Same with a fragment after — strip fragment before checking.
        assert!(looks_truncated("https://example.com/?foo#section"));
    }

    #[test]
    fn looks_truncated_silent_on_clean_url() {
        // Normal URLs (no query, or query with at least one `=`) are clean.
        assert!(!looks_truncated("https://example.com/"));
        assert!(!looks_truncated("https://example.com/path/to/page"));
        assert!(!looks_truncated("https://example.com/?a=1"));
        assert!(!looks_truncated("https://example.com/?a=1&b=2"));
        assert!(!looks_truncated(
            "https://example.com/?a=1&b=2&c=hello%20world"
        ));
        // Hash anchors without a query are clean.
        assert!(!looks_truncated("https://example.com/page#section"));
    }

    #[test]
    fn looks_truncated_silent_with_url_encoded_assertion_modeled_via_skip() {
        // The --url-encoded flag suppresses the warning at the call site
        // (main.rs gates the eprintln! behind `if !cli.url_encoded`).
        // This test models the gate logic directly: when --url-encoded is set,
        // the warning branch is never entered, even on a truncated-looking URL.
        let url = "https://example.com/?a=1&";
        let url_encoded_flag = true;
        let should_warn = !url_encoded_flag && looks_truncated(url);
        assert!(
            !should_warn,
            "--url-encoded must suppress the warning even on URL ending with &"
        );

        // Sanity: same URL without --url-encoded does warn.
        let url_encoded_flag = false;
        let should_warn = !url_encoded_flag && looks_truncated(url);
        assert!(
            should_warn,
            "without --url-encoded, the warning should fire on URL ending with &"
        );
    }

    #[test]
    fn research_slug_truncation_is_char_safe() {
        // Multibyte query: byte-slicing at 50 would panic mid-codepoint.
        let query = "日本語".repeat(40); // 120 chars, 3 bytes each
        let slug: String = query
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-")
            .to_lowercase();
        let slug: String = slug.chars().take(50).collect();
        assert!(slug.chars().count() <= 50);
        // Round-trips through formatting without panicking.
        let _ = format!("research-{slug}.json");
    }

    #[test]
    fn a_capture_pattern_needs_an_equals_sign() {
        let cli =
            Cli::try_parse_from(["webclaw", "--capture-network", "https://example.com"]).unwrap();
        assert_eq!(cli.capture_network.as_deref(), Some(""));
        assert_eq!(cli.urls, ["https://example.com"]);
        let cli =
            Cli::try_parse_from(["webclaw", "--capture-network=/api/", "https://example.com"])
                .unwrap();
        assert_eq!(cli.capture_network.as_deref(), Some("/api/"));
    }

    #[test]
    fn a_capture_refuses_page_options_it_would_ignore() {
        let parse = |extra: &[&str]| {
            let mut args = vec!["webclaw", "https://example.com", "--capture-network"];
            args.extend_from_slice(extra);
            Cli::try_parse_from(args)
        };
        assert!(parse(&[]).is_ok());
        for extra in [
            &["-f", "markdown"][..],
            &["--query", "price"],
            &["--chunk", "200"],
            &["--include", "main"],
            &["--exclude", "nav"],
            &["--only-main-content"],
            &["--metadata"],
        ] {
            assert!(parse(extra).is_err(), "{extra:?} was accepted");
        }
    }
}
