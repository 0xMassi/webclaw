//! webclaw-fetch: HTTP client layer with browser TLS fingerprint impersonation.
//! Uses wreq (BoringSSL) for browser-grade TLS + HTTP/2 fingerprinting.
//! Automatically detects PDF responses and delegates to webclaw-pdf.
#[cfg(feature = "browser")]
mod actions;
mod block;
pub mod browser;
mod cache;
pub mod client;
pub mod cloud;
pub mod crawler;
pub mod document;
pub mod error;
pub mod extractors;
pub mod fetcher;
pub mod linkedin;
pub mod locale;
pub mod map;
pub mod progress;
pub mod proxy;
pub mod reddit;
#[cfg(feature = "browser")]
pub mod render;
pub mod robots;
pub mod search;
pub mod sitemap;
pub mod tls;
pub mod transfer;
pub mod url_security;
mod xml;

pub use block::Block;
pub use browser::BrowserProfile;
pub use client::{BatchExtractResult, BatchResult, FetchClient, FetchConfig, FetchResult};
pub use crawler::{CrawlConfig, CrawlResult, CrawlState, Crawler, PageResult, PageSink};
pub use error::FetchError;
pub use fetcher::Fetcher;
pub use http::HeaderMap;
pub use locale::{accept_language_for_tld, accept_language_for_url};
pub use map::{MapOptions, discover_urls};
pub use progress::{PROGRESS_INTERVAL, with_progress};
pub use proxy::{parse_proxy_file, parse_proxy_line};
pub use search::{SearchOptions, SearchResult, parse_serper_organic, search};
pub use sitemap::SitemapEntry;
pub use webclaw_pdf::PdfMode;
