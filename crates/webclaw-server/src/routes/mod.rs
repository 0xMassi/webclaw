//! HTTP route handlers.
//!
//! The OSS server exposes a deliberately small surface that mirrors the
//! hosted-API JSON shapes where the underlying capability exists in the
//! OSS crates. Endpoints that depend on private infrastructure
//! (anti-bot bypass with stealth Chrome, JS rendering at scale,
//! per-user auth, billing, async job queues, agent loops) are
//! intentionally not implemented here. Use api.webclaw.io for those.
//!
//! `POST /v1/search` is supported when the operator supplies their own
//! Serper.dev API key via the `SERPER_API_KEY` env var (free key at
//! serper.dev). Without it, the route returns 501. This is the
//! bring-your-own-key path — no hosted webclaw account required.

pub mod batch;
pub mod brand;
pub mod crawl;
pub mod diff;
pub mod extract;
pub mod health;
pub mod map;
pub mod scrape;
pub mod search;
pub mod structured;
pub mod summarize;
