use std::process::Command;

#[test]
fn an_unknown_block_type_is_refused_even_without_rendering() {
    let out = Command::new(env!("CARGO_BIN_EXE_webclaw"))
        .args([
            "http://example.com/",
            "--render",
            "never",
            "--block",
            "bogus",
            "-p",
            "http://127.0.0.1:1",
        ])
        .env_remove("WEBCLAW_API_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot block \"bogus\""),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
