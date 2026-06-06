//! Async run handlers for every CLI mode: crawl, map, batch, watch, diff,
//! brand, LLM extraction/summarization, and cloud research.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use webclaw_core::{ChangeStatus, ExtractionOptions, ExtractionResult};
use webclaw_fetch::{CrawlConfig, Crawler, FetchClient, PageResult};
use webclaw_llm::LlmProvider;

use crate::cli::{Cli, OutputFormat};
use crate::fetch::{
    EmptyReason, build_extraction_options, build_fetch_config, detect_empty,
    enrich_html_with_stylesheets, fetch_and_extract, fetch_html, normalize_url, url_to_filename,
    warn_empty, write_to_file,
};
use crate::output::{
    format_output, format_progress, print_batch_output, print_crawl_output, print_diff_output,
    print_map_output,
};
use crate::webhook::{fire_webhook, spawn_on_change};

pub async fn run_crawl(cli: &Cli) -> Result<(), String> {
    let url = cli
        .urls
        .first()
        .ok_or("--crawl requires a URL argument")
        .map(|u| normalize_url(u))?;
    let url = url.as_str();

    if cli.file.is_some() || cli.stdin {
        return Err("--crawl cannot be used with --file or --stdin".into());
    }

    let include_patterns: Vec<String> = cli
        .include_paths
        .as_deref()
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();
    let exclude_patterns: Vec<String> = cli
        .exclude_paths
        .as_deref()
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();

    // Set up streaming progress channel
    let (progress_tx, mut progress_rx) = tokio::sync::broadcast::channel::<PageResult>(100);

    // Set up cancel flag for Ctrl+C handling
    let cancel_flag = Arc::new(AtomicBool::new(false));

    // Register Ctrl+C handler when --crawl-state is set
    let state_path = cli.crawl_state.clone();
    if state_path.is_some() {
        let flag = Arc::clone(&cancel_flag);
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            flag.store(true, Ordering::Relaxed);
            eprintln!("\nCtrl+C received, saving crawl state...");
        });
    }

    let config = CrawlConfig {
        fetch: build_fetch_config(cli),
        max_depth: cli.depth,
        max_pages: cli.max_pages,
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
    };

    // Load resume state if --crawl-state file exists
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

    let max_pages = cli.max_pages;
    let completed_offset = resume_state.as_ref().map_or(0, |s| s.completed_pages);

    // Spawn background task to print streaming progress to stderr
    let progress_handle = tokio::spawn(async move {
        let mut count = completed_offset;
        while let Ok(page) = progress_rx.recv().await {
            count += 1;
            eprintln!("{}", format_progress(&page, count, max_pages));
        }
    });

    let crawler = Crawler::new(url, config).map_err(|e| format!("crawler error: {e}"))?;
    let result = crawler.crawl(url, resume_state).await;

    // Drop the crawler (and its progress_tx clone) so the progress task finishes
    drop(crawler);
    let _ = progress_handle.await;

    // If cancelled via Ctrl+C and --crawl-state is set, save state for resume
    let was_cancelled = cancel_flag.load(Ordering::Relaxed);
    if was_cancelled {
        if let Some(ref path) = state_path {
            Crawler::save_state(
                path,
                url,
                &result.visited,
                &result.remaining_frontier,
                completed_offset + result.pages.len(),
                cli.max_pages,
                cli.depth,
            )?;
            eprintln!(
                "Crawl state saved to {} ({} pages completed). Resume with --crawl-state {}",
                path.display(),
                completed_offset + result.pages.len(),
                path.display(),
            );
        }
    } else if let Some(ref path) = state_path {
        // Crawl completed normally — clean up state file
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }

    // Log per-page errors and extraction warnings to stderr
    for page in &result.pages {
        if let Some(ref err) = page.error {
            eprintln!("error: {} -- {}", page.url, err);
        } else if let Some(ref extraction) = page.extraction {
            let reason = detect_empty(extraction);
            if !matches!(reason, EmptyReason::None) {
                warn_empty(&page.url, &reason);
            }
        }
    }

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
    } else {
        print_crawl_output(&result, &cli.format, cli.metadata);
    }

    eprintln!(
        "Crawled {} pages ({} ok, {} errors) in {:.1}s",
        result.total, result.ok, result.errors, result.elapsed_secs,
    );

    // Fire webhook on crawl complete
    if let Some(ref webhook_url) = cli.webhook {
        let urls: Vec<&str> = result.pages.iter().map(|p| p.url.as_str()).collect();
        fire_webhook(
            webhook_url,
            &serde_json::json!({
                "event": "crawl_complete",
                "total": result.total,
                "ok": result.ok,
                "errors": result.errors,
                "elapsed_secs": result.elapsed_secs,
                "urls": urls,
            }),
        );
        // Brief pause so the async webhook has time to fire
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
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

pub async fn run_map(cli: &Cli) -> Result<(), String> {
    let url = cli
        .urls
        .first()
        .ok_or("--map requires a URL argument")
        .map(|u| normalize_url(u))?;
    let url = url.as_str();

    let client =
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?;

    // Layered discovery: sitemaps first, bounded crawl fallback when thin.
    let mut opts = webclaw_fetch::MapOptions::default();
    if let Some(pages) = cli.map_pages {
        opts.max_crawl_pages = pages;
    }
    if cli.no_map_crawl {
        opts.crawl_fallback = false;
    }
    if let Some(limit) = cli.map_limit {
        opts.max_urls = Some(limit);
    }

    let entries = webclaw_fetch::discover_urls(&client, url, &opts).await;

    if entries.is_empty() {
        eprintln!("no URLs found for {url}");
    } else {
        eprintln!("discovered {} URLs", entries.len());
    }

    print_map_output(&entries, &cli.format);
    Ok(())
}

pub async fn run_batch(cli: &Cli, entries: &[(String, Option<String>)]) -> Result<(), String> {
    let client = Arc::new(
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?,
    );

    let urls: Vec<&str> = entries.iter().map(|(u, _)| u.as_str()).collect();
    let options = build_extraction_options(cli);
    let results = client
        .fetch_and_extract_batch_with_options(&urls, cli.concurrency, &options)
        .await;

    let ok = results.iter().filter(|r| r.result.is_ok()).count();
    let errors = results.len() - ok;

    // Log errors and extraction warnings to stderr
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

    // Build a lookup of custom filenames by URL
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
    } else {
        print_batch_output(&results, &cli.format, cli.metadata);
    }

    eprintln!(
        "Fetched {} URLs ({} ok, {} errors)",
        results.len(),
        ok,
        errors
    );

    // Fire webhook on batch complete
    if let Some(ref webhook_url) = cli.webhook {
        let urls: Vec<&str> = results.iter().map(|r| r.url.as_str()).collect();
        fire_webhook(
            webhook_url,
            &serde_json::json!({
                "event": "batch_complete",
                "total": results.len(),
                "ok": ok,
                "errors": errors,
                "urls": urls,
            }),
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
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

pub async fn run_watch(cli: &Cli, urls: &[String]) -> Result<(), String> {
    if urls.is_empty() {
        return Err("--watch requires at least one URL".into());
    }

    let client = Arc::new(
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?,
    );
    let options = build_extraction_options(cli);

    // Ctrl+C handler
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        flag.store(true, Ordering::Relaxed);
    });

    // Single-URL mode: preserve original behavior exactly
    if urls.len() == 1 {
        return run_watch_single(cli, &client, &options, &urls[0], &cancelled).await;
    }

    // Multi-URL mode: batch fetch, diff each, report aggregate
    run_watch_multi(cli, &client, &options, urls, &cancelled).await
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

pub async fn run_diff(cli: &Cli, snapshot_path: &str) -> Result<(), String> {
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

pub async fn run_brand(cli: &Cli) -> Result<(), String> {
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
                "unknown LLM provider: {other} (use ollama, openai, or anthropic)"
            )),
        }
    } else {
        let chain = webclaw_llm::ProviderChain::default().await;
        if chain.is_empty() {
            return Err(
                "no LLM providers available -- start Ollama or set OPENAI_API_KEY / ANTHROPIC_API_KEY"
                    .into(),
            );
        }
        Ok(Box::new(chain))
    }
}

pub async fn run_llm(cli: &Cli) -> Result<(), String> {
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
            &result.content.plain_text,
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
pub async fn run_batch_llm(cli: &Cli, entries: &[(String, Option<String>)]) -> Result<(), String> {
    let client =
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?;
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
            webclaw_llm::summarize::summarize(text, Some(sentences), provider.as_ref(), model)
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

    eprintln!("Processed {total} URLs ({ok} ok, {errors} errors)");

    if let Some(ref webhook_url) = cli.webhook {
        fire_webhook(
            webhook_url,
            &serde_json::json!({
                "event": "batch_llm_complete",
                "total": total,
                "ok": ok,
                "errors": errors,
            }),
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
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
pub fn has_llm_flags(cli: &Cli) -> bool {
    cli.extract_json.is_some() || cli.extract_prompt.is_some() || cli.summarize.is_some()
}

pub async fn run_research(cli: &Cli, query: &str) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
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
}
