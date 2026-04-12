// Pipeline — filesystem watcher → chunk → embed → upsert
//
// Architecture:
//   notify-debouncer-mini (sync mpsc) → spawn_blocking bridge → tokio mpsc IndexJob queue
//   → embed_concurrency worker tasks → process_job()
//
// Key design decisions:
//   - Carry tracing::Span in IndexJob; tokio::spawn would drop it otherwise.
//   - Per-URL mutex (DashMap<String, Arc<Mutex<()>>>) prevents concurrent delete+upsert races.
//   - Workers bounded to embed_concurrency provide natural backpressure without a separate semaphore.
//   - notify-debouncer-mini 0.4.x uses a callback/sender API, not a receiver() method.
//     We use std::sync::mpsc::Sender<DebounceEventResult> as the handler and bridge via spawn_blocking.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use noxa_core::types::ExtractionResult;
use tokenizers::Tokenizer;

use crate::chunker;
use crate::config::{RagConfig, SourceConfig};
use crate::embed::DynEmbedProvider;
use crate::error::RagError;
use crate::store::DynVectorStore;
use crate::types::{Point, PointPayload};

// ─── IndexJob ────────────────────────────────────────────────────────────────

/// A unit of work: index the .json file at `path`.
/// The tracing `span` is carried explicitly because tokio::spawn does NOT
/// automatically propagate the current span into the new task.
struct IndexJob {
    path: PathBuf,
    span: tracing::Span,
}

// ─── Pipeline ────────────────────────────────────────────────────────────────

pub struct Pipeline {
    pub config: RagConfig,
    pub embed: DynEmbedProvider,
    pub store: DynVectorStore,
    pub tokenizer: Arc<Tokenizer>,
    pub shutdown: CancellationToken,
    /// Per-URL mutex: prevents concurrent delete-then-upsert races for the same URL.
    url_locks: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Pipeline {
    pub fn new(
        config: RagConfig,
        embed: DynEmbedProvider,
        store: DynVectorStore,
        tokenizer: Arc<Tokenizer>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            config,
            embed,
            store,
            tokenizer,
            shutdown,
            url_locks: Arc::new(DashMap::new()),
        }
    }

    /// Run the filesystem watcher pipeline.
    ///
    /// Returns when the CancellationToken is cancelled.
    pub async fn run(&self) -> Result<(), RagError> {
        // Extract watch config.
        let (watch_dir, debounce_ms) = match &self.config.source {
            SourceConfig::FsWatcher { watch_dir, debounce_ms } => {
                (watch_dir.clone(), *debounce_ms)
            }
        };

        tracing::info!(
            watch_dir = %watch_dir.display(),
            debounce_ms,
            embed_concurrency = self.config.pipeline.embed_concurrency,
            "pipeline starting"
        );

        // Bounded job queue: backpressure at 256 queued jobs.
        let (tx, rx) = tokio::sync::mpsc::channel::<IndexJob>(256);

        // Spawn worker pool — each worker owns a cloned rx.
        // We share a single receiver via Arc<Mutex<Receiver>> so all workers
        // compete fairly for jobs.
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        let mut worker_handles = Vec::with_capacity(self.config.pipeline.embed_concurrency);

        for worker_id in 0..self.config.pipeline.embed_concurrency {
            let rx = rx.clone();
            let embed = self.embed.clone();
            let store = self.store.clone();
            let tokenizer = self.tokenizer.clone();
            let config = self.config.clone();
            let url_locks = self.url_locks.clone();

            let handle = tokio::spawn(async move {
                tracing::debug!(worker_id, "index worker started");
                loop {
                    let job = {
                        let mut guard = rx.lock().await;
                        guard.recv().await
                    };
                    match job {
                        Some(job) => {
                            let span = job.span.clone();
                            async {
                                if let Err(e) = process_job(
                                    job,
                                    &embed,
                                    &store,
                                    &tokenizer,
                                    &config,
                                    &url_locks,
                                )
                                .await
                                {
                                    tracing::error!(error = %e, "index job failed");
                                }
                            }
                            .instrument(span)
                            .await;
                        }
                        None => {
                            // Sender dropped — workers drain and exit.
                            tracing::debug!(worker_id, "index worker shutting down");
                            break;
                        }
                    }
                }
            });

            worker_handles.push(handle);
        }

        // Build notify debouncer with std::sync::mpsc sender as the event handler.
        // notify-debouncer-mini 0.4.x implements DebounceEventHandler for
        // std::sync::mpsc::Sender<DebounceEventResult> out of the box.
        let (notify_tx, notify_rx) = std::sync::mpsc::channel::<DebounceEventResult>();

        let mut debouncer =
            new_debouncer(Duration::from_millis(debounce_ms), notify_tx).map_err(|e| {
                RagError::Generic(format!("failed to create fs watcher: {e}"))
            })?;

        debouncer
            .watcher()
            .watch(&watch_dir, RecursiveMode::NonRecursive)
            .map_err(|e| {
                RagError::Generic(format!(
                    "failed to watch directory {}: {e}",
                    watch_dir.display()
                ))
            })?;

        tracing::info!(path = %watch_dir.display(), "watching directory (non-recursive)");

        // Bridge: wrap the blocking notify_rx.recv() in spawn_blocking so it
        // doesn't block the tokio reactor.  Send jobs to the tokio job queue.
        let shutdown_clone = self.shutdown.clone();
        let tx_clone = tx.clone();

        let bridge_handle = tokio::task::spawn_blocking(move || {
            // Keep `debouncer` alive for the duration of this thread.
            let _debouncer = debouncer;

            loop {
                // recv_timeout lets us periodically check whether we should stop.
                // We check every 250 ms regardless of debounce setting.
                match notify_rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(Ok(events)) => {
                        if shutdown_clone.is_cancelled() {
                            break;
                        }
                        for event in events {
                            let path = event.path;
                            if !is_indexable(&path) {
                                continue;
                            }
                            let span = tracing::info_span!(
                                "index_job",
                                path = %path.display(),
                            );
                            let job = IndexJob { path, span };
                            match tx_clone.try_send(job) {
                                Ok(_) => {}
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                    tracing::warn!("index job queue full, dropping event");
                                }
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                    // Workers are done; exit.
                                    break;
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(error = ?e, "fs watcher error");
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Check if we should stop.
                        if shutdown_clone.is_cancelled() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }

            tracing::info!("fs watcher bridge exiting");
        });

        // Wait for cancellation signal.
        self.shutdown.cancelled().await;
        tracing::info!("shutdown signal received, draining pipeline");

        // Drop tx so workers drain their queues and exit.
        drop(tx);

        // Wait for bridge to finish.
        let _ = bridge_handle.await;

        // Wait for all workers to finish.
        for handle in worker_handles {
            let _ = handle.await;
        }

        tracing::info!("pipeline shut down cleanly");
        Ok(())
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Returns true iff the path has a `.json` extension AND exists on disk.
/// We check both because rename events (vim/emacs atomic saves) may fire for
/// temp files that are gone by the time we process them.
fn is_indexable(path: &Path) -> bool {
    path.extension().map(|e| e == "json").unwrap_or(false) && path.exists()
}

/// Returns true iff `host` resolves to a private/loopback/link-local address.
fn is_private_ip(host: &str) -> bool {
    if let Ok(addr) = host.parse::<IpAddr>() {
        return match addr {
            IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
            IpAddr::V6(ip) => ip.is_loopback(),
        };
    }
    false
}

/// Validate that `url` uses http or https and does not point to a private IP.
fn validate_url_scheme(url: &str) -> Result<(), RagError> {
    if url.is_empty() {
        return Err(RagError::Generic("extraction result has no URL".to_string()));
    }
    let parsed = url::Url::parse(url)
        .map_err(|e| RagError::Generic(format!("invalid URL {url:?}: {e}")))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(RagError::Generic(format!(
                "URL scheme {other:?} is not allowed (only http/https)"
            )));
        }
    }

    if let Some(host) = parsed.host_str() {
        if is_private_ip(host) {
            return Err(RagError::Generic(format!(
                "URL {url:?} resolves to a private/loopback IP — indexing blocked"
            )));
        }
        // Also block bare "localhost" hostname.
        if host.eq_ignore_ascii_case("localhost") {
            return Err(RagError::Generic(
                "URL points to localhost — indexing blocked".to_string(),
            ));
        }
    }

    Ok(())
}

/// Append a failed-job record to the configured log file (NDJSON format).
/// Silently ignores if no log path is configured.
async fn append_failed_job(path: &Path, error: &impl std::fmt::Display, config: &RagConfig) {
    let Some(ref log_path) = config.pipeline.failed_jobs_log else {
        return;
    };
    let entry = serde_json::json!({
        "path": path.to_string_lossy(),
        "error": error.to_string(),
        "ts": chrono::Utc::now().to_rfc3339(),
    });
    if let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .await
    {
        use tokio::io::AsyncWriteExt;
        let _ = file
            .write_all(format!("{}\n", entry).as_bytes())
            .await;
    }
}

// ─── Core processing ─────────────────────────────────────────────────────────

async fn process_job(
    job: IndexJob,
    embed: &DynEmbedProvider,
    store: &DynVectorStore,
    tokenizer: &Arc<Tokenizer>,
    config: &RagConfig,
    url_locks: &Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
) -> Result<(), RagError> {
    // ── 1. Open file and check size from the same FD (TOCTOU fix) ────────────
    let mut file = tokio::fs::File::open(&job.path).await?;
    let size = file.metadata().await?.len();

    const MAX_FILE_SIZE_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB
    if size > MAX_FILE_SIZE_BYTES {
        tracing::warn!(
            path = ?job.path,
            size,
            "file too large (>50MB), skipping"
        );
        return Ok(());
    }

    let mut content = String::with_capacity(size as usize);
    file.read_to_string(&mut content).await?;

    // ── 2. Parse JSON ─────────────────────────────────────────────────────────
    let result: ExtractionResult = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(path = ?job.path, error = %e, "json parse failed, skipping");
            append_failed_job(&job.path, &e, config).await;
            return Ok(());
        }
    };

    // ── 3. URL validation ─────────────────────────────────────────────────────
    let url = result.metadata.url.as_deref().unwrap_or("").to_string();
    if let Err(e) = validate_url_scheme(&url) {
        tracing::warn!(path = ?job.path, error = %e, "url validation failed, skipping");
        return Ok(());
    }

    // ── 4. Chunk ──────────────────────────────────────────────────────────────
    let chunks = chunker::chunk(&result, &config.chunker, tokenizer);
    if chunks.is_empty() {
        tracing::info!(url = %url, "no indexable content after chunking");
        return Ok(());
    }

    // ── 5. Embed ──────────────────────────────────────────────────────────────
    let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
    let vectors = embed.embed(&texts).await?;

    if vectors.len() != chunks.len() {
        return Err(RagError::Embed(format!(
            "embed returned {} vectors for {} chunks",
            vectors.len(),
            chunks.len()
        )));
    }

    // ── 6. Build points with deterministic UUID v5 ────────────────────────────
    let points: Vec<Point> = chunks
        .iter()
        .zip(vectors.iter())
        .enumerate()
        .map(|(i, (chunk, vector))| {
            let id = uuid::Uuid::new_v5(
                &config.uuid_namespace,
                format!("{}#chunk{}", chunk.source_url, i).as_bytes(),
            );
            Point {
                id,
                vector: vector.clone(),
                payload: PointPayload {
                    text: chunk.text.clone(),
                    url: chunk.source_url.clone(),
                    domain: chunk.domain.clone(),
                    chunk_index: chunk.chunk_index,
                    total_chunks: chunk.total_chunks,
                    token_estimate: chunk.token_estimate,
                },
            }
        })
        .collect();

    // ── 7. Per-URL mutex: delete-then-upsert under lock ───────────────────────
    let url_lock = url_locks
        .entry(url.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = url_lock.lock().await;

    store.delete_by_url(&url).await?;
    store.upsert(points).await?;

    drop(_guard);

    // ── 8. Done ───────────────────────────────────────────────────────────────
    tracing::info!(url = %url, chunks = texts.len(), "indexed");
    Ok(())
}
