//! Offline extraction timings. See benchmarks/extraction.md for workloads and comparisons.
use std::{env, fs, hint::black_box, time::Instant};

use scraper::Html;
use webclaw_core::{ExtractionOptions, extract_with_options, extractor, structured_data};

fn measure<T>(iterations: usize, mut operation: impl FnMut() -> T) -> Vec<u128> {
    for _ in 0..5 {
        black_box(operation());
    }
    (0..iterations)
        .map(|_| {
            let start = Instant::now();
            drop(black_box(operation()));
            start.elapsed().as_nanos()
        })
        .collect()
}

fn main() {
    let args: Vec<_> = env::args().collect();
    assert_eq!(
        args.len(),
        6,
        "usage: extraction_bench MODE FILE URL|- ITERATIONS OUTPUT_JSON"
    );
    let html = fs::read_to_string(&args[2]).expect("read HTML fixture");
    let url = (args[3] != "-").then_some(args[3].as_str());
    let iterations: usize = args[4].parse().expect("positive iteration count");
    assert!(iterations > 0);
    let options = ExtractionOptions::default();
    let samples = match args[1].as_str() {
        "extract" => measure(iterations, || {
            extract_with_options(&html, url, &options).unwrap()
        }),
        "jsonld" => measure(iterations, || structured_data::extract_json_ld(&html)),
        "parse" => measure(iterations, || Html::parse_document(&html)),
        "content" => {
            let doc = Html::parse_document(&html);
            let base = url.map(|u| url::Url::parse(u).unwrap());
            measure(iterations, || {
                extractor::extract_content(&doc, base.as_ref(), &options)
            })
        }
        "output" => {
            let variants = [
                options,
                ExtractionOptions {
                    only_main_content: true,
                    ..Default::default()
                },
                ExtractionOptions {
                    include_raw_html: true,
                    ..Default::default()
                },
                ExtractionOptions {
                    include_selectors: vec!["body".into()],
                    exclude_selectors: vec!["nav".into(), "footer".into()],
                    ..Default::default()
                },
            ];
            let outputs: Vec<_> = variants
                .iter()
                .map(|opts| {
                    extract_with_options(&html, url, opts)
                        .map(|result| serde_json::to_value(result).unwrap())
                        .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}))
                })
                .collect();
            fs::write(&args[5], serde_json::to_vec(&outputs).unwrap()).unwrap();
            return;
        }
        _ => panic!("mode must be extract, jsonld, parse, content, or output"),
    };
    let result = serde_json::json!({
        "mode": args[1], "file": args[2], "url": url, "bytes": html.len(),
        "iterations": iterations, "samples_ns": samples,
    });
    fs::write(&args[5], serde_json::to_vec_pretty(&result).unwrap()).unwrap();
}
