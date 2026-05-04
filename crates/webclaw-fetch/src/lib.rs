//! webclaw-fetch: HTTP client layer with browser TLS fingerprint impersonation.
//! Uses wreq (BoringSSL) for browser-grade TLS + HTTP/2 fingerprinting.
//! Automatically detects PDF responses and delegates to webclaw-pdf.
pub mod browser;
pub mod client;
pub mod cloud;
pub mod crawler;
pub mod document;
pub mod error;
pub mod extractors;
pub mod fetcher;
pub mod linkedin;
pub mod locale;
pub mod proxy;
pub mod reddit;
pub mod sitemap;
pub mod tls;
pub mod url_security;

pub use browser::BrowserProfile;
pub use client::{BatchExtractResult, BatchResult, FetchClient, FetchConfig, FetchResult};
pub use crawler::{CrawlConfig, CrawlResult, CrawlState, Crawler, PageResult};
pub use error::FetchError;
pub use fetcher::Fetcher;
pub use http::HeaderMap;
pub use locale::{accept_language_for_tld, accept_language_for_url};
pub use proxy::{parse_proxy_file, parse_proxy_line};
pub use sitemap::SitemapEntry;
pub use webclaw_pdf::PdfMode;
