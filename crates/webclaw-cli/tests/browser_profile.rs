#![cfg(feature = "browser")]

use std::process::{Command, Stdio};

fn chromes_on(profile: &std::path::Path) -> usize {
    let ps = Command::new("/bin/ps")
        .args(["-axo", "command="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&ps.stdout)
        .lines()
        .filter(|line| line.contains(&format!("--user-data-dir={}", profile.display())))
        .count()
}

#[test]
#[ignore = "launches Chrome and needs network access"]
fn a_failed_fetch_leaves_no_chrome_on_the_profile() {
    let profile = std::env::temp_dir().join(format!("webclaw-test-profile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    let webclaw = |url: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_webclaw"));
        command
            .args([url, "-f", "text", "--browser-profile"])
            .arg(&profile)
            .env_remove("WEBCLAW_API_KEY");
        command
    };
    let failed = webclaw("https://webclaw-test.invalid/")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!failed.success());
    let left = chromes_on(&profile);
    let next = webclaw("https://example.com/")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    let _ = std::fs::remove_dir_all(&profile);
    assert_eq!(left, 0, "Chrome left running on the profile");
    assert!(next.success(), "the next run on the profile failed");
}
