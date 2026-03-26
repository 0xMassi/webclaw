/// webclaw-fetch: HTTP client layer with browser TLS fingerprint impersonation.
/// Uses Impit under the hood to make requests that look like real
/// browsers at the TLS, HTTP/2, and header levels.
/// Automatically detects PDF responses and delegates to webclaw-pdf.
pub mod browser;
pub mod client;
pub mod crawler;
pub mod document;
pub mod error;
pub mod linkedin;
pub mod proxy;
pub mod reddit;
pub mod sitemap;

pub use browser::BrowserProfile;
pub use client::{BatchExtractResult, BatchResult, FetchClient, FetchConfig, FetchResult};
pub use crawler::{CrawlConfig, CrawlResult, CrawlState, Crawler, PageResult};
pub use error::FetchError;
pub use proxy::{parse_proxy_file, parse_proxy_line};
pub use sitemap::SitemapEntry;
pub use webclaw_pdf::PdfMode;
