//! HTTP route handlers.
//!
//! The OSS server exposes a deliberately small surface that mirrors the
//! hosted-API JSON shapes where the underlying capability exists in the
//! OSS crates. Endpoints that depend on private infrastructure
//! (anti-bot bypass with stealth Chrome, JS rendering at scale,
//! per-user auth, billing, async job queues, agent loops) are
//! intentionally not implemented here. Use api.webclaw.io for those.

pub mod batch;
pub mod brand;
pub mod capture;
pub mod crawl;
pub mod diff;
pub mod extract;
pub mod health;
pub mod map;
pub mod scrape;
pub mod structured;
pub mod summarize;
