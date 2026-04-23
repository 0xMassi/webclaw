//! Browser TLS + HTTP/2 fingerprint profiles built on wreq (BoringSSL).
//!
//! Replaces the old webclaw-http/webclaw-tls patched rustls stack.
//! Each profile configures TLS options (cipher suites, curves, extensions,
//! PSK, ECH GREASE) and HTTP/2 options (SETTINGS order, pseudo-header order,
//! stream dependency, priorities) to match real browser fingerprints.

use std::time::Duration;

use std::borrow::Cow;

use wreq::http2::{
    Http2Options, PseudoId, PseudoOrder, SettingId, SettingsOrder, StreamDependency, StreamId,
};
use wreq::tls::{
    AlpnProtocol, AlpsProtocol, CertificateCompressionAlgorithm, ExtensionType, TlsOptions,
    TlsVersion,
};
use wreq::{Client, Emulation};

use crate::browser::BrowserVariant;
use crate::error::FetchError;

/// Chrome cipher list (TLS 1.3 + TLS 1.2 in Chrome's exact order).
const CHROME_CIPHERS: &str = "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_AES_128_GCM_SHA256:TLS_RSA_WITH_AES_256_GCM_SHA384:TLS_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_CBC_SHA";

/// Chrome signature algorithms.
const CHROME_SIGALGS: &str = "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:rsa_pss_rsae_sha512:rsa_pkcs1_sha512";

/// Chrome curves (post-quantum ML-KEM + X25519 + P-256 + P-384).
const CHROME_CURVES: &str = "X25519MLKEM768:X25519:P-256:P-384";

/// Firefox cipher list.
const FIREFOX_CIPHERS: &str = "TLS_AES_128_GCM_SHA256:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_256_GCM_SHA384:TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_AES_128_GCM_SHA256:TLS_RSA_WITH_AES_256_GCM_SHA384:TLS_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_CBC_SHA";

/// Firefox signature algorithms.
const FIREFOX_SIGALGS: &str = "ecdsa_secp256r1_sha256:ecdsa_secp384r1_sha384:ecdsa_secp521r1_sha512:rsa_pss_rsae_sha256:rsa_pss_rsae_sha384:rsa_pss_rsae_sha512:rsa_pkcs1_sha256:rsa_pkcs1_sha384:rsa_pkcs1_sha512:ecdsa_sha1:rsa_pkcs1_sha1";

/// Firefox curves.
const FIREFOX_CURVES: &str = "X25519MLKEM768:X25519:P-256:P-384:P-521";

/// Safari cipher list.
const SAFARI_CIPHERS: &str = "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:TLS_RSA_WITH_AES_256_GCM_SHA384:TLS_RSA_WITH_AES_128_GCM_SHA256:TLS_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_AES_128_CBC_SHA";

/// Safari signature algorithms.
const SAFARI_SIGALGS: &str = "ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:ecdsa_secp521r1_sha512:rsa_pss_rsae_sha512:rsa_pkcs1_sha384:rsa_pkcs1_sha512";

/// Safari curves.
const SAFARI_CURVES: &str = "X25519:P-256:P-384:P-521";

/// Safari iOS 26 TLS extension order, matching bogdanfinn's
/// `safari_ios_26_0` wire format. GREASE slots are omitted. wreq
/// inserts them itself. Diverges from wreq-util's default SafariIos26
/// extension order, which DataDome's immobiliare.it ruleset flags.
fn safari_ios_extensions() -> Vec<ExtensionType> {
    vec![
        ExtensionType::CERTIFICATE_TIMESTAMP,
        ExtensionType::APPLICATION_LAYER_PROTOCOL_NEGOTIATION,
        ExtensionType::SERVER_NAME,
        ExtensionType::CERT_COMPRESSION,
        ExtensionType::KEY_SHARE,
        ExtensionType::SUPPORTED_VERSIONS,
        ExtensionType::PSK_KEY_EXCHANGE_MODES,
        ExtensionType::SUPPORTED_GROUPS,
        ExtensionType::RENEGOTIATE,
        ExtensionType::SIGNATURE_ALGORITHMS,
        ExtensionType::STATUS_REQUEST,
        ExtensionType::EC_POINT_FORMATS,
        ExtensionType::EXTENDED_MASTER_SECRET,
    ]
}

/// Chrome 133 TLS extension order, matching bogdanfinn's stable JA3
/// (`43067709b025da334de1279a120f8e14`). Real Chrome permutes extensions
/// per handshake, but indeed.com's WAF allowlists this specific wire order
/// and rejects permuted ones. GREASE slots are inserted by wreq.
///
/// JA3 extension field from peet.ws: 18-5-35-51-10-45-11-27-17613-43-13-0-16-65037-65281-23
fn chrome_extensions() -> Vec<ExtensionType> {
    vec![
        ExtensionType::CERTIFICATE_TIMESTAMP,                  // 18
        ExtensionType::STATUS_REQUEST,                         // 5
        ExtensionType::SESSION_TICKET,                         // 35
        ExtensionType::KEY_SHARE,                              // 51
        ExtensionType::SUPPORTED_GROUPS,                       // 10
        ExtensionType::PSK_KEY_EXCHANGE_MODES,                 // 45
        ExtensionType::EC_POINT_FORMATS,                       // 11
        ExtensionType::CERT_COMPRESSION,                       // 27
        ExtensionType::APPLICATION_SETTINGS_NEW, // 17613 (new codepoint, matches alps_use_new_codepoint)
        ExtensionType::SUPPORTED_VERSIONS,       // 43
        ExtensionType::SIGNATURE_ALGORITHMS,     // 13
        ExtensionType::SERVER_NAME,              // 0
        ExtensionType::APPLICATION_LAYER_PROTOCOL_NEGOTIATION, // 16
        ExtensionType::ENCRYPTED_CLIENT_HELLO,   // 65037
        ExtensionType::RENEGOTIATE,              // 65281
        ExtensionType::EXTENDED_MASTER_SECRET,   // 23
    ]
}

// --- Chrome HTTP headers in correct wire order ---

const CHROME_HEADERS: &[(&str, &str)] = &[
    (
        "sec-ch-ua",
        r#""Google Chrome";v="145", "Chromium";v="145", "Not/A)Brand";v="24""#,
    ),
    ("sec-ch-ua-mobile", "?0"),
    ("sec-ch-ua-platform", "\"Windows\""),
    ("upgrade-insecure-requests", "1"),
    (
        "user-agent",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36",
    ),
    (
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
    ),
    ("sec-fetch-site", "none"),
    ("sec-fetch-mode", "navigate"),
    ("sec-fetch-user", "?1"),
    ("sec-fetch-dest", "document"),
    ("accept-encoding", "gzip, deflate, br, zstd"),
    ("accept-language", "en-US,en;q=0.9"),
    ("priority", "u=0, i"),
];

const CHROME_MACOS_HEADERS: &[(&str, &str)] = &[
    (
        "sec-ch-ua",
        r#""Google Chrome";v="145", "Chromium";v="145", "Not/A)Brand";v="24""#,
    ),
    ("sec-ch-ua-mobile", "?0"),
    ("sec-ch-ua-platform", "\"macOS\""),
    ("upgrade-insecure-requests", "1"),
    (
        "user-agent",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36",
    ),
    (
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
    ),
    ("sec-fetch-site", "none"),
    ("sec-fetch-mode", "navigate"),
    ("sec-fetch-user", "?1"),
    ("sec-fetch-dest", "document"),
    ("accept-encoding", "gzip, deflate, br, zstd"),
    ("accept-language", "en-US,en;q=0.9"),
    ("priority", "u=0, i"),
];

const FIREFOX_HEADERS: &[(&str, &str)] = &[
    (
        "user-agent",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:135.0) Gecko/20100101 Firefox/135.0",
    ),
    (
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    ),
    ("accept-language", "en-US,en;q=0.5"),
    ("accept-encoding", "gzip, deflate, br, zstd"),
    ("upgrade-insecure-requests", "1"),
    ("sec-fetch-dest", "document"),
    ("sec-fetch-mode", "navigate"),
    ("sec-fetch-site", "none"),
    ("sec-fetch-user", "?1"),
    ("priority", "u=0, i"),
];

const SAFARI_HEADERS: &[(&str, &str)] = &[
    (
        "user-agent",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3.1 Safari/605.1.15",
    ),
    (
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    ),
    ("sec-fetch-site", "none"),
    ("accept-language", "en-US,en;q=0.9"),
    ("sec-fetch-mode", "navigate"),
    ("accept-encoding", "gzip, deflate, br"),
    ("sec-fetch-dest", "document"),
];

/// Safari iOS 26 headers, in the wire order real Safari emits. Critically:
/// NO `sec-fetch-*`, NO `priority: u=0, i` (both Chromium-only leaks), but
/// `upgrade-insecure-requests: 1` is present. `accept-encoding` does not
/// include zstd (Safari can't decode it). Verified against bogdanfinn on
/// 2026-04-22: this header set is what DataDome's immobiliare ruleset
/// expects for a real iPhone.
const SAFARI_IOS_HEADERS: &[(&str, &str)] = &[
    (
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    ),
    ("accept-language", "en-US,en;q=0.9"),
    ("accept-encoding", "gzip, deflate, br"),
    (
        "user-agent",
        "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1",
    ),
    ("upgrade-insecure-requests", "1"),
];

const EDGE_HEADERS: &[(&str, &str)] = &[
    (
        "sec-ch-ua",
        r#""Microsoft Edge";v="145", "Chromium";v="145", "Not/A)Brand";v="24""#,
    ),
    ("sec-ch-ua-mobile", "?0"),
    ("sec-ch-ua-platform", "\"Windows\""),
    ("upgrade-insecure-requests", "1"),
    (
        "user-agent",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36 Edg/145.0.0.0",
    ),
    (
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
    ),
    ("sec-fetch-site", "none"),
    ("sec-fetch-mode", "navigate"),
    ("sec-fetch-user", "?1"),
    ("sec-fetch-dest", "document"),
    ("accept-encoding", "gzip, deflate, br, zstd"),
    ("accept-language", "en-US,en;q=0.9"),
    ("priority", "u=0, i"),
];

fn chrome_tls() -> TlsOptions {
    // permute_extensions is off so the explicit extension_permutation sticks.
    // Real Chrome permutes, but indeed.com's WAF allowlists bogdanfinn's
    // fixed order, so matching that gets us through.
    TlsOptions::builder()
        .cipher_list(CHROME_CIPHERS)
        .sigalgs_list(CHROME_SIGALGS)
        .curves_list(CHROME_CURVES)
        .min_tls_version(TlsVersion::TLS_1_2)
        .max_tls_version(TlsVersion::TLS_1_3)
        .grease_enabled(true)
        .permute_extensions(false)
        .extension_permutation(chrome_extensions())
        .enable_ech_grease(true)
        .pre_shared_key(true)
        .enable_ocsp_stapling(true)
        .enable_signed_cert_timestamps(true)
        .alpn_protocols([
            AlpnProtocol::HTTP3,
            AlpnProtocol::HTTP2,
            AlpnProtocol::HTTP1,
        ])
        .alps_protocols([AlpsProtocol::HTTP3, AlpsProtocol::HTTP2])
        .alps_use_new_codepoint(true)
        .aes_hw_override(true)
        .certificate_compression_algorithms(&[CertificateCompressionAlgorithm::BROTLI])
        .build()
}

fn firefox_tls() -> TlsOptions {
    TlsOptions::builder()
        .cipher_list(FIREFOX_CIPHERS)
        .sigalgs_list(FIREFOX_SIGALGS)
        .curves_list(FIREFOX_CURVES)
        .min_tls_version(TlsVersion::TLS_1_2)
        .max_tls_version(TlsVersion::TLS_1_3)
        .grease_enabled(true)
        .permute_extensions(false)
        .enable_ech_grease(true)
        .pre_shared_key(true)
        .enable_ocsp_stapling(true)
        .enable_signed_cert_timestamps(true)
        .certificate_compression_algorithms(&[
            CertificateCompressionAlgorithm::ZLIB,
            CertificateCompressionAlgorithm::BROTLI,
        ])
        .build()
}

fn safari_tls() -> TlsOptions {
    TlsOptions::builder()
        .cipher_list(SAFARI_CIPHERS)
        .sigalgs_list(SAFARI_SIGALGS)
        .curves_list(SAFARI_CURVES)
        .min_tls_version(TlsVersion::TLS_1_2)
        .max_tls_version(TlsVersion::TLS_1_3)
        .grease_enabled(true)
        .permute_extensions(false)
        .enable_ech_grease(false)
        .pre_shared_key(false)
        .enable_ocsp_stapling(true)
        .enable_signed_cert_timestamps(true)
        .certificate_compression_algorithms(&[CertificateCompressionAlgorithm::ZLIB])
        .build()
}

/// Safari iOS 26 emulation — composed on top of `wreq_util::Emulation::SafariIos26`
/// with four targeted overrides. We don't hand-roll this one like Chrome/Firefox
/// because the wire-level defaults from wreq-util are already correct for ciphers,
/// sigalgs, curves, and GREASE — the four things wreq-util gets *wrong* for
/// DataDome compatibility are overridden here:
///
///  1. TLS extension order: match bogdanfinn `safari_ios_26_0` exactly (JA3
///     ends up `8d909525bd5bbb79f133d11cc05159fe`).
///  2. HTTP/2 HEADERS priority flag: weight=256, exclusive=1, depends_on=0.
///     wreq-util omits this frame; real Safari and bogdanfinn include it.
///     This flip is the thing DataDome actually reads — the akamai_fingerprint
///     hash changes from `c52879e43202aeb92740be6e8c86ea96` to
///     `d1294410a06522e37a5c5e3f0a45a705`, which is the winning signature.
///  3. Headers: strip wreq-util's Chromium defaults (`sec-fetch-*`,
///     `priority: u=0, i`, zstd), replace with the real iOS 26 set.
///  4. `accept-language` preserved from config.extra_headers for locale.
fn safari_ios_emulation() -> wreq::Emulation {
    use wreq::EmulationFactory;
    let mut em = wreq_util::Emulation::SafariIos26.emulation();

    if let Some(tls) = em.tls_options_mut().as_mut() {
        tls.extension_permutation = Some(Cow::Owned(safari_ios_extensions()));
    }

    // Only override the priority flag — keep wreq-util's SETTINGS, WINDOW_UPDATE,
    // and pseudo-order intact. Replacing the whole Http2Options resets SETTINGS
    // to defaults, which sends only INITIAL_WINDOW_SIZE and fails DataDome.
    if let Some(h2) = em.http2_options_mut().as_mut() {
        h2.headers_stream_dependency = Some(StreamDependency::new(StreamId::zero(), 255, true));
    }

    let hm = em.headers_mut();
    hm.clear();
    for (k, v) in SAFARI_IOS_HEADERS {
        if let (Ok(n), Ok(val)) = (
            http::header::HeaderName::from_bytes(k.as_bytes()),
            http::header::HeaderValue::from_str(v),
        ) {
            hm.append(n, val);
        }
    }

    em
}

fn chrome_h2() -> Http2Options {
    // SETTINGS frame matches bogdanfinn `chrome_133`: HEADER_TABLE_SIZE,
    // ENABLE_PUSH=0, INITIAL_WINDOW_SIZE, MAX_HEADER_LIST_SIZE. No
    // MAX_CONCURRENT_STREAMS — real Chrome 133 and bogdanfinn both omit it,
    // and indeed.com's WAF reads this as a bot signal when present. Priority
    // weight 256 (encoded as 255 + 1) matches bogdanfinn's HEADERS frame.
    Http2Options::builder()
        .initial_window_size(6_291_456)
        .initial_connection_window_size(15_728_640)
        .max_header_list_size(262_144)
        .header_table_size(65_536)
        .enable_push(false)
        .settings_order(
            SettingsOrder::builder()
                .extend([
                    SettingId::HeaderTableSize,
                    SettingId::EnablePush,
                    SettingId::InitialWindowSize,
                    SettingId::MaxHeaderListSize,
                ])
                .build(),
        )
        .headers_pseudo_order(
            PseudoOrder::builder()
                .extend([
                    PseudoId::Method,
                    PseudoId::Authority,
                    PseudoId::Scheme,
                    PseudoId::Path,
                ])
                .build(),
        )
        .headers_stream_dependency(StreamDependency::new(StreamId::zero(), 255, true))
        .build()
}

fn firefox_h2() -> Http2Options {
    Http2Options::builder()
        .initial_window_size(131_072)
        .initial_connection_window_size(12_517_377)
        .max_header_list_size(65_536)
        .header_table_size(65_536)
        .settings_order(
            SettingsOrder::builder()
                .extend([
                    SettingId::HeaderTableSize,
                    SettingId::InitialWindowSize,
                    SettingId::MaxFrameSize,
                ])
                .build(),
        )
        .headers_pseudo_order(
            PseudoOrder::builder()
                .extend([
                    PseudoId::Method,
                    PseudoId::Path,
                    PseudoId::Authority,
                    PseudoId::Scheme,
                ])
                .build(),
        )
        .build()
}

fn safari_h2() -> Http2Options {
    Http2Options::builder()
        .initial_window_size(2_097_152)
        .initial_connection_window_size(10_420_225)
        .max_header_list_size(0)
        .header_table_size(4_096)
        .enable_push(false)
        .max_concurrent_streams(100u32)
        .settings_order(
            SettingsOrder::builder()
                .extend([
                    SettingId::EnablePush,
                    SettingId::MaxConcurrentStreams,
                    SettingId::InitialWindowSize,
                    SettingId::MaxFrameSize,
                ])
                .build(),
        )
        .headers_pseudo_order(
            PseudoOrder::builder()
                .extend([
                    PseudoId::Method,
                    PseudoId::Scheme,
                    PseudoId::Authority,
                    PseudoId::Path,
                ])
                .build(),
        )
        .headers_stream_dependency(StreamDependency::new(StreamId::zero(), 255, false))
        .build()
}

fn build_headers(pairs: &[(&str, &str)]) -> http::HeaderMap {
    let mut map = http::HeaderMap::with_capacity(pairs.len());
    for (name, value) in pairs {
        if let (Ok(n), Ok(v)) = (
            http::header::HeaderName::from_bytes(name.as_bytes()),
            http::header::HeaderValue::from_str(value),
        ) {
            map.insert(n, v);
        }
    }
    map
}

/// Build a wreq Client for a specific browser variant.
pub fn build_client(
    variant: BrowserVariant,
    timeout: Duration,
    extra_headers: &std::collections::HashMap<String, String>,
    proxy: Option<&str>,
) -> Result<Client, FetchError> {
    // SafariIos26 builds its Emulation on top of wreq-util's base instead
    // of from scratch. See `safari_ios_emulation` for why.
    let mut emulation = match variant {
        BrowserVariant::SafariIos26 => safari_ios_emulation(),
        other => {
            let (tls, h2, headers) = match other {
                BrowserVariant::Chrome => (chrome_tls(), chrome_h2(), CHROME_HEADERS),
                BrowserVariant::ChromeMacos => (chrome_tls(), chrome_h2(), CHROME_MACOS_HEADERS),
                BrowserVariant::Firefox => (firefox_tls(), firefox_h2(), FIREFOX_HEADERS),
                BrowserVariant::Safari => (safari_tls(), safari_h2(), SAFARI_HEADERS),
                BrowserVariant::Edge => (chrome_tls(), chrome_h2(), EDGE_HEADERS),
                BrowserVariant::SafariIos26 => unreachable!("handled above"),
            };
            Emulation::builder()
                .tls_options(tls)
                .http2_options(h2)
                .headers(build_headers(headers))
                .build()
        }
    };

    // Append extra headers after profile defaults.
    let hm = emulation.headers_mut();
    for (k, v) in extra_headers {
        if let (Ok(n), Ok(val)) = (
            http::header::HeaderName::from_bytes(k.as_bytes()),
            http::header::HeaderValue::from_str(v),
        ) {
            hm.insert(n, val);
        }
    }

    let mut builder = Client::builder()
        .emulation(emulation)
        .redirect(wreq::redirect::Policy::limited(10))
        .cookie_store(true)
        .timeout(timeout);

    if let Some(proxy_url) = proxy {
        let proxy =
            wreq::Proxy::all(proxy_url).map_err(|e| FetchError::Build(format!("proxy: {e}")))?;
        builder = builder.proxy(proxy);
    }

    builder
        .build()
        .map_err(|e| FetchError::Build(e.to_string()))
}
