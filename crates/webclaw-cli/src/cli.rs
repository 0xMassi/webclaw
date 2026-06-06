//! CLI argument definitions: clap structs/enums and their conversions.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use webclaw_fetch::BrowserProfile;
use webclaw_pdf::PdfMode;

#[derive(Parser)]
#[command(name = "webclaw", about = "Extract web content for LLMs", version)]
pub struct Cli {
    /// Optional subcommand. When omitted, the CLI falls back to the
    /// traditional flag-based flow (URL + --format, --crawl, etc.).
    /// Subcommands are used for flows that don't fit that model.
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// URLs to fetch (multiple allowed)
    #[arg()]
    pub urls: Vec<String>,

    /// File with URLs (one per line)
    #[arg(long)]
    pub urls_file: Option<String>,

    /// Output format (markdown, json, text, llm, html)
    #[arg(short, long, default_value = "markdown")]
    pub format: OutputFormat,

    /// Browser to impersonate
    #[arg(short, long, default_value = "chrome")]
    pub browser: Browser,

    /// Proxy URL (http://user:pass@host:port or socks5://host:port)
    #[arg(short, long, env = "WEBCLAW_PROXY")]
    pub proxy: Option<String>,

    /// File with proxies (host:port:user:pass, one per line). Rotates per request.
    #[arg(long, env = "WEBCLAW_PROXY_FILE")]
    pub proxy_file: Option<String>,

    /// Request timeout in seconds
    #[arg(short, long, default_value = "30")]
    pub timeout: u64,

    /// Extract from local HTML file instead of fetching
    #[arg(long)]
    pub file: Option<String>,

    /// Read HTML from stdin
    #[arg(long)]
    pub stdin: bool,

    /// Include metadata in output (always included in JSON)
    #[arg(long)]
    pub metadata: bool,

    /// Output raw fetched HTML instead of extracting
    #[arg(long)]
    pub raw_html: bool,

    /// CSS selectors to include (comma-separated, e.g. "article,.content")
    #[arg(long)]
    pub include: Option<String>,

    /// CSS selectors to exclude (comma-separated, e.g. "nav,.sidebar,footer")
    #[arg(long)]
    pub exclude: Option<String>,

    /// Only extract main content (article/main element)
    #[arg(long)]
    pub only_main_content: bool,

    /// Custom headers (repeatable, e.g. -H "Cookie: foo=bar")
    #[arg(short = 'H', long = "header")]
    pub headers: Vec<String>,

    /// Cookie string (shorthand for -H "Cookie: ...")
    #[arg(long)]
    pub cookie: Option<String>,

    /// JSON cookie file (Chrome extension format: [{name, value, domain, ...}])
    #[arg(long)]
    pub cookie_file: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    pub verbose: bool,

    /// Compare against a previous JSON snapshot
    #[arg(long)]
    pub diff_with: Option<String>,

    /// Watch a URL for changes. Checks at the specified interval and reports diffs.
    #[arg(long)]
    pub watch: bool,

    /// Watch interval in seconds [default: 300]
    #[arg(long, default_value = "300")]
    pub watch_interval: u64,

    /// Command to run when changes are detected (receives diff JSON on stdin)
    #[arg(long)]
    pub on_change: Option<String>,

    /// Webhook URL: POST a JSON payload when an operation completes.
    /// Works with crawl, batch, watch (on change), and single URL modes.
    #[arg(long, env = "WEBCLAW_WEBHOOK_URL")]
    pub webhook: Option<String>,

    /// Extract brand identity (colors, fonts, logo)
    #[arg(long)]
    pub brand: bool,

    // -- PDF options --
    /// PDF extraction mode: auto (error on empty) or fast (return whatever text is found)
    #[arg(long, default_value = "auto")]
    pub pdf_mode: PdfModeArg,

    // -- Crawl options --
    /// Enable recursive crawling of same-domain links
    #[arg(long)]
    pub crawl: bool,

    /// Max crawl depth [default: 1]
    #[arg(long, default_value = "1")]
    pub depth: usize,

    /// Max pages to crawl [default: 20]
    #[arg(long, default_value = "20")]
    pub max_pages: usize,

    /// Max concurrent requests [default: 5]
    #[arg(long, default_value = "5")]
    pub concurrency: usize,

    /// Delay between requests in ms [default: 100]
    #[arg(long, default_value = "100")]
    pub delay: u64,

    /// Only crawl URLs matching this path prefix
    #[arg(long)]
    pub path_prefix: Option<String>,

    /// Glob patterns for crawl URL paths to include (comma-separated, e.g. "/api/*,/guides/**")
    #[arg(long)]
    pub include_paths: Option<String>,

    /// Glob patterns for crawl URL paths to exclude (comma-separated, e.g. "/changelog/*,/blog/*")
    #[arg(long)]
    pub exclude_paths: Option<String>,

    /// Path to save/resume crawl state. On Ctrl+C: saves progress. On start: resumes if file exists.
    #[arg(long)]
    pub crawl_state: Option<PathBuf>,

    /// Seed crawl frontier from sitemap discovery (robots.txt + /sitemap.xml)
    #[arg(long)]
    pub sitemap: bool,

    /// Discover URLs from sitemap and print them (one per line; JSON array with --format json)
    #[arg(long)]
    pub map: bool,

    /// Max pages for --map's crawl fallback when the sitemap is thin [default: 150]
    #[arg(long)]
    pub map_pages: Option<usize>,

    /// Disable --map's crawl fallback (sitemap-only discovery)
    #[arg(long)]
    pub no_map_crawl: bool,

    /// Cap the number of URLs --map returns (default: uncapped)
    #[arg(long)]
    pub map_limit: Option<usize>,

    // -- LLM options --
    /// Extract structured JSON using LLM (pass a JSON schema string or @file)
    #[arg(long)]
    pub extract_json: Option<String>,

    /// Extract using natural language prompt
    #[arg(long)]
    pub extract_prompt: Option<String>,

    /// Summarize content using LLM (optional: number of sentences, default 3)
    #[arg(long, num_args = 0..=1, default_missing_value = "3")]
    pub summarize: Option<usize>,

    /// Force a specific LLM provider (ollama, openai, anthropic)
    #[arg(long, env = "WEBCLAW_LLM_PROVIDER")]
    pub llm_provider: Option<String>,

    /// Override the LLM model name
    #[arg(long, env = "WEBCLAW_LLM_MODEL")]
    pub llm_model: Option<String>,

    /// Override the LLM base URL (Ollama, OpenAI-compatible, or Anthropic-compatible)
    #[arg(long, env = "WEBCLAW_LLM_BASE_URL")]
    pub llm_base_url: Option<String>,

    // -- Cloud API options --
    /// Webclaw Cloud API key for automatic fallback on bot-protected or JS-rendered sites
    #[arg(long, env = "WEBCLAW_API_KEY")]
    pub api_key: Option<String>,

    /// Force all requests through the cloud API (skip local extraction)
    #[arg(long)]
    pub cloud: bool,

    /// Run deep research on a topic via the cloud API. Requires --api-key.
    /// Saves full result (report + sources + findings) to a JSON file.
    #[arg(long)]
    pub research: Option<String>,

    /// Enable deep research mode (longer, more thorough report). Used with --research.
    #[arg(long)]
    pub deep: bool,

    /// Output directory: save each page to a separate file instead of stdout.
    /// Works with --crawl, batch (multiple URLs), and single URL mode.
    /// Filenames are derived from URL paths (e.g. /docs/api -> docs/api.md).
    #[arg(long)]
    pub output_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Commands {
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
        #[arg(long, env = "SERPER_API_KEY")]
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
pub enum OutputFormat {
    Markdown,
    Json,
    Text,
    Llm,
    Html,
}

impl OutputFormat {
    /// Map to the cloud API's `formats` string. Single source of truth for the
    /// format names the REST API expects.
    pub fn as_api_str(&self) -> &'static str {
        match self {
            OutputFormat::Markdown => "markdown",
            OutputFormat::Json => "json",
            OutputFormat::Text => "text",
            OutputFormat::Llm => "llm",
            OutputFormat::Html => "html",
        }
    }
}

#[derive(Clone, ValueEnum)]
pub enum Browser {
    Chrome,
    Firefox,
    /// Safari iOS 26. Pair with a country-matched residential proxy for sites
    /// that reject non-mobile profiles.
    SafariIos,
    Random,
}

#[derive(Clone, ValueEnum, Default)]
pub enum PdfModeArg {
    /// Error if PDF has no extractable text (catches scanned PDFs)
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
            Browser::Firefox => BrowserProfile::Firefox,
            Browser::SafariIos => BrowserProfile::SafariIos,
            Browser::Random => BrowserProfile::Random,
        }
    }
}
