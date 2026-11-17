use webclaw_fetch::{BrowserProfile, FetchClient, FetchConfig};

const ECHO: &str = "https://tls.peet.ws/api/all";
const CHROME_H2: &str = "1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p";
const CHROME_ORDER: &[&str] = &[
    "sec-ch-ua",
    "sec-ch-ua-mobile",
    "sec-ch-ua-platform",
    "upgrade-insecure-requests",
    "user-agent",
    "accept",
    "sec-fetch-site",
    "sec-fetch-mode",
    "sec-fetch-user",
    "sec-fetch-dest",
    "accept-encoding",
    "accept-language",
    "priority",
];

async fn echo(browser: BrowserProfile) -> serde_json::Value {
    let client = FetchClient::new(FetchConfig {
        browser,
        ..FetchConfig::default()
    })
    .expect("client builds");
    let page = client.fetch(ECHO).await.expect("echo service answers");
    serde_json::from_str(&page.html).expect("echo service returns JSON")
}

fn sent_headers(echo: &serde_json::Value) -> Vec<(String, String)> {
    echo["http2"]["sent_frames"]
        .as_array()
        .and_then(|frames| frames.iter().find(|f| f["frame_type"] == "HEADERS"))
        .and_then(|f| f["headers"].as_array())
        .expect("a HEADERS frame")
        .iter()
        .filter_map(|h| h.as_str()?.split_once(": "))
        .filter(|(name, _)| !name.starts_with(':'))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[tokio::test]
#[ignore = "talks to a public TLS echo service"]
async fn chromium_profiles_look_like_chrome_on_the_wire() {
    for (profile, alpn) in [
        (BrowserProfile::Chrome, "h3"),
        (BrowserProfile::ChromeMacos, "h3"),
        (BrowserProfile::Edge, "h3"),
        (BrowserProfile::Chrome149, "h2"),
        (BrowserProfile::ChromeMacos149, "h2"),
        (BrowserProfile::Edge149, "h2"),
    ] {
        let echo = echo(profile.clone()).await;
        let ja4 = echo["tls"]["ja4"].as_str().unwrap_or_default();
        assert!(
            ja4.starts_with("t13d1516h2_8daaf6152771_"),
            "{profile:?} {ja4}"
        );
        assert_eq!(
            echo["http2"]["akamai_fingerprint"], CHROME_H2,
            "{profile:?}"
        );
        let offered = echo["tls"]["extensions"]
            .as_array()
            .and_then(|e| {
                e.iter()
                    .find(|e| e["name"] == "application_layer_protocol_negotiation (16)")
            })
            .map(|e| e["protocols"][0].clone());
        assert_eq!(offered, Some(alpn.into()), "{profile:?}");
        let headers = sent_headers(&echo);
        let names: Vec<_> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, CHROME_ORDER, "{profile:?}");
        if alpn == "h2" {
            let agent = &headers.iter().find(|(n, _)| n == "user-agent").unwrap().1;
            for (name, value) in webclaw_fetch::tls::chrome_client_hints(agent).unwrap() {
                assert!(
                    headers.contains(&(name.to_string(), value.clone())),
                    "{profile:?} {name}"
                );
            }
        }
    }
}

#[tokio::test]
#[ignore = "talks to a public TLS echo service"]
async fn firefox_151_sends_no_grease_and_firefox_h2_settings() {
    let echo = echo(BrowserProfile::Firefox151).await;
    let ja4 = echo["tls"]["ja4"].as_str().unwrap_or_default();
    assert!(ja4.starts_with("t13d1717h2_"), "{ja4}");
    assert!(
        echo["http2"]["akamai_fingerprint"]
            .as_str()
            .is_some_and(|fp| fp.ends_with("|m,p,a,s"))
    );
    let grease = echo["tls"]["extensions"].as_array().is_some_and(|e| {
        e.iter()
            .any(|e| e["name"].as_str().is_some_and(|n| n.contains("GREASE")))
    });
    assert!(!grease);
}
