//! SSRF guard for every server-side fetch.
//!
//! Callers may still do cheap parse validation at the edge, but this
//! module is the fetch-layer authority because redirects and helper
//! fetches also pass through it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use tokio::net::lookup_host;
use url::{Host, Url};

use crate::error::FetchError;

/// Parse a caller-provided URL and require an HTTP(S) host.
pub fn validate_http_url(raw: &str) -> Result<Url, FetchError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(FetchError::InvalidUrl("URL must not be empty".into()));
    }

    let parsed =
        Url::parse(trimmed).map_err(|e| FetchError::InvalidUrl(format!("invalid URL: {e}")))?;
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(FetchError::InvalidUrl(format!(
                "scheme '{scheme}' is not allowed, use http:// or https://"
            )));
        }
    }

    if parsed.host().is_none() {
        return Err(FetchError::InvalidUrl("URL must include a host".into()));
    }

    Ok(parsed)
}

/// Parse, resolve, and reject private/internal destinations.
///
/// A domain is rejected if any resolved address is private or reserved.
/// That is intentionally conservative: mixed public/private DNS answers
/// are unsafe for server-side fetching.
pub async fn validate_public_http_url(raw: &str) -> Result<Url, FetchError> {
    let parsed = validate_http_url(raw)?;
    validate_url_host_is_public(&parsed).await?;
    Ok(parsed)
}

async fn validate_url_host_is_public(url: &Url) -> Result<(), FetchError> {
    match url.host() {
        Some(Host::Ipv4(ip)) => reject_blocked_ip(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => reject_blocked_ip(IpAddr::V6(ip)),
        Some(Host::Domain(host)) => {
            let port = url
                .port_or_known_default()
                .ok_or_else(|| FetchError::InvalidUrl("URL must include a known port".into()))?;
            let addrs = lookup_host((host, port))
                .await
                .map_err(|e| FetchError::InvalidUrl(format!("failed to resolve host: {e}")))?;

            let mut resolved = false;
            for addr in addrs {
                resolved = true;
                reject_blocked_ip(addr.ip())?;
            }
            if !resolved {
                return Err(FetchError::InvalidUrl(
                    "host did not resolve to any addresses".into(),
                ));
            }
            Ok(())
        }
        None => Err(FetchError::InvalidUrl("URL must include a host".into())),
    }
}

fn reject_blocked_ip(ip: IpAddr) -> Result<(), FetchError> {
    if is_blocked_ip(ip) {
        Err(FetchError::InvalidUrl(
            "URL resolves to a blocked private or internal address".into(),
        ))
    } else {
        Ok(())
    }
}

/// Return true for IP ranges that should never be fetched server-side.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();

    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || o[0] == 0
        || o[0] >= 224
        || (o[0] == 100 && (64..=127).contains(&o[1]))
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && (18..=19).contains(&o[1]))
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();

    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0)
        || (s[0] == 0x2001 && s[1] == 0x0db8)
        || embedded_ipv4(ip).is_some_and(is_blocked_ipv4)
}

fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();

    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0xffff {
        return Some(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
    }

    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
        return Some(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::{is_blocked_ip, validate_public_http_url};

    #[tokio::test]
    async fn blocks_ipv4_internal_ranges() {
        for ip in [
            Ipv4Addr::new(0, 0, 0, 0),
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(100, 64, 0, 1),
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(169, 254, 169, 254),
            Ipv4Addr::new(172, 16, 0, 1),
            Ipv4Addr::new(192, 168, 0, 1),
            Ipv4Addr::new(198, 18, 0, 1),
        ] {
            let url = format!("http://{ip}/");
            assert!(validate_public_http_url(&url).await.is_err(), "{ip}");
        }
    }

    #[tokio::test]
    async fn blocks_ipv6_internal_ranges() {
        for ip in [
            Ipv6Addr::LOCALHOST,
            Ipv6Addr::UNSPECIFIED,
            "fc00::1".parse().unwrap(),
            "fe80::1".parse().unwrap(),
            "64:ff9b::7f00:1".parse().unwrap(),
            "::ffff:127.0.0.1".parse().unwrap(),
        ] {
            assert!(is_blocked_ip(IpAddr::V6(ip)), "{ip}");
        }
    }

    #[tokio::test]
    async fn allows_public_ip_literals() {
        assert!(
            validate_public_http_url("https://93.184.216.34/")
                .await
                .is_ok()
        );
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))) == false);
    }
}
