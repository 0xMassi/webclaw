use std::io::Write;
use std::process::{Command, Stdio};

fn chunks(args: &[&str], stdin: Option<&str>) -> Vec<serde_json::Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_webclaw"))
        .args(args)
        .env_remove("WEBCLAW_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(html) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(html.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn chunk_prints_one_json_object_per_chunk() {
    let paragraph = "Cargo builds the code and fetches the crates it needs. ".repeat(12);
    let html = format!(
        "<html><body><article><h1>Rust</h1><p>Rust is a language.</p><h2>Tooling</h2><p>{paragraph}</p><h2>History</h2><p>Graydon Hoare began it in 2006.</p></article></body></html>"
    );
    let chunks = chunks(&["--stdin", "--chunk", "60"], Some(&html));
    assert!(chunks.len() > 2, "{chunks:?}");
    for (i, c) in chunks.iter().enumerate() {
        assert_eq!(c["index"], i);
        assert!(c["tokens"].as_u64().unwrap() <= 60, "{c}");
        assert!(c["heading_path"][0] == "Rust", "{c}");
    }
    assert!(
        chunks
            .iter()
            .any(|c| c["heading_path"] == serde_json::json!(["Rust", "Tooling"])),
        "{chunks:?}"
    );
}

#[test]
#[ignore = "fetches a public page"]
fn chunk_a_live_page_keeps_its_facts_whole_and_its_url() {
    let url = "https://books.toscrape.com/catalogue/a-light-in-the-attic_1000/index.html";
    let chunks = chunks(&[url, "--render", "never", "--chunk", "64"], None);
    assert!(chunks.len() > 2, "{chunks:?}");
    assert!(chunks.iter().all(|c| c["url"] == url));
    for fact in ["A Light in the Attic", "51.77", "a897fe39b1053632"] {
        assert!(
            chunks
                .iter()
                .any(|c| c["text"].as_str().unwrap().contains(fact)),
            "{fact}"
        );
    }
}
