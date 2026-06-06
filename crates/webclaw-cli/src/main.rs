/// CLI entry point -- wires webclaw-core and webclaw-fetch into a single command.
/// All extraction and fetching logic lives in sibling crates and modules; this
/// file is the argument parser plus dispatch.
mod bench;
mod cli;
mod fetch;
mod output;
mod run;
mod webhook;

use std::process;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Commands};
use fetch::{
    FetchOutput, collect_urls, fetch_and_extract, fetch_html, normalize_url, url_to_filename,
    write_to_file,
};
use output::{format_output, print_cloud_output, print_output};
use run::{
    has_llm_flags, run_batch, run_batch_llm, run_brand, run_crawl, run_diff, run_llm, run_map,
    run_research, run_search, run_watch,
};

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

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    init_logging(cli.verbose);

    // Subcommand path. Handled before the flag dispatch so a subcommand
    // can't collide with a flag-based flow. When no subcommand is set
    // we fall through to the existing behaviour.
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
                    process::exit(1);
                }
                return;
            }
            Commands::Extractors { json } => {
                let entries = webclaw_fetch::extractors::list();
                if *json {
                    // Serialize with serde_json. ExtractorInfo derives
                    // Serialize so this is a one-liner.
                    match serde_json::to_string_pretty(&entries) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("error: failed to serialise catalog: {e}");
                            process::exit(1);
                        }
                    }
                } else {
                    // Human-friendly table: NAME + LABEL + one URL
                    // pattern sample. Keeps the output scannable on a
                    // narrow terminal.
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
                // Build a FetchClient with cloud fallback attached when
                // WEBCLAW_API_KEY is set. Antibot-gated verticals
                // (amazon, ebay, etsy, trustpilot) need this to escalate
                // on bot protection.
                let fetch_cfg = webclaw_fetch::FetchConfig {
                    browser: webclaw_fetch::BrowserProfile::Firefox,
                    ..webclaw_fetch::FetchConfig::default()
                };
                let mut client = match webclaw_fetch::FetchClient::new(fetch_cfg) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("error: failed to build fetch client: {e}");
                        process::exit(1);
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
                                process::exit(1);
                            }
                        }
                    }
                    Err(e) => {
                        // UrlMismatch / UnknownVertical / Fetch all get
                        // Display impls with actionable messages.
                        eprintln!("error: {e}");
                        process::exit(1);
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
                        process::exit(1);
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
                    process::exit(1);
                }
                return;
            }
        }
    }

    // --map: sitemap discovery mode
    if cli.map {
        if let Err(e) = run_map(&cli).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --crawl: recursive crawl mode
    if cli.crawl {
        if let Err(e) = run_crawl(&cli).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --watch: poll URL(s) for changes
    if cli.watch {
        let watch_urls: Vec<String> = match collect_urls(&cli) {
            Ok(entries) => entries.into_iter().map(|(url, _)| url).collect(),
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
        };
        if let Err(e) = run_watch(&cli, &watch_urls).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --diff-with: change tracking mode
    if let Some(ref snapshot_path) = cli.diff_with {
        if let Err(e) = run_diff(&cli, snapshot_path).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --brand: brand identity extraction mode
    if cli.brand {
        if let Err(e) = run_brand(&cli).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --research: deep research via cloud API
    if let Some(ref query) = cli.research {
        if let Err(e) = run_research(&cli, query).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // Collect all URLs from args + --urls-file
    let entries = match collect_urls(&cli) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    // LLM modes: --extract-json, --extract-prompt, --summarize
    // When multiple URLs are provided, run batch LLM extraction over all of them.
    if has_llm_flags(&cli) {
        if entries.len() > 1 {
            if let Err(e) = run_batch_llm(&cli, &entries).await {
                eprintln!("error: {e}");
                process::exit(1);
            }
        } else if let Err(e) = run_llm(&cli).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // Multi-URL batch mode
    if entries.len() > 1 {
        if let Err(e) = run_batch(&cli, &entries).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --raw-html: skip extraction, dump the fetched HTML
    if cli.raw_html && cli.include.is_none() && cli.exclude.is_none() {
        match fetch_html(&cli).await {
            Ok(r) => println!("{}", r.html),
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        return;
    }

    // Single-page extraction (handles both HTML and PDF via content-type detection)
    match fetch_and_extract(&cli).await {
        Ok(FetchOutput::Local(result)) => {
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
                    process::exit(1);
                }
            } else {
                print_output(&result, &cli.format, cli.metadata);
            }
        }
        Ok(FetchOutput::Cloud(resp)) => {
            print_cloud_output(&resp, &cli.format);
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(1);
        }
    }
}
