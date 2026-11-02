//! SSRF guard for every server-side fetch.
//!
//! Callers may still do cheap parse validation at the edge, but this
//! module is the fetch-layer authority because redirects and helper
//! fetches also pass through it.
//!
//! `WEBCLAW_ALLOW_CIDRS` (comma-separated CIDRs or addresses, read once)
//! lifts the block for the ranges it lists, for networks where public
//! names resolve to reserved addresses, such as the fake-IP DNS of a TUN
//! proxy handing out 198.18.0.0/15. Loopback, unspecified, link-local,
//! multicast and broadcast addresses and the cloud metadata addresses
//! stay blocked whatever it lists.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

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
            "URL resolves to a blocked private or internal address \
             (WEBCLAW_ALLOW_CIDRS lists ranges to allow)"
                .into(),
        ))
    } else {
        Ok(())
    }
}

/// Return true for IP ranges that should never be fetched server-side,
/// less the ranges `WEBCLAW_ALLOW_CIDRS` lists (see the module docs).
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    is_blocked(ip, allowed_cidrs())
}

fn is_blocked(ip: IpAddr, allowed: &[Cidr]) -> bool {
    let blocked = match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    };
    blocked && (never_allowed(ip) || !allowed.iter().any(|cidr| cidr.contains(ip)))
}

const ALIBABA_METADATA: Ipv4Addr = Ipv4Addr::new(100, 100, 100, 200);
const AWS_METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

fn never_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_link_local()
                || o[0] == 0
                || o[0] >= 224
                || ip == ALIBABA_METADATA
        }
        IpAddr::V6(ip) => {
            ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || (ip.segments()[0] & 0xffc0) == 0xfe80
                || ip == AWS_METADATA_V6
                || embedded_ipv4(ip)
                    .or_else(|| nat64_ipv4(ip))
                    .is_some_and(|v4| never_allowed(IpAddr::V4(v4)))
        }
    }
}

fn nat64_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    (s[..6] == [0x0064, 0xff9b, 0, 0, 0, 0])
        .then(|| Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8))
}

#[derive(Debug, Clone, Copy)]
struct Cidr {
    net: IpAddr,
    prefix: u8,
}

impl Cidr {
    fn contains(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 => v4,
        };
        let (net, ip, bits) = match (self.net, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => (u32::from(net).into(), u32::from(ip).into(), 32),
            (IpAddr::V6(net), IpAddr::V6(ip)) => (u128::from(net), u128::from(ip), 128),
            _ => return false,
        };
        let shift = bits - u32::from(self.prefix);
        shift == 128 || net >> shift == ip >> shift
    }
}

fn parse_cidr(entry: &str) -> Option<Cidr> {
    let entry = entry.trim();
    let (addr, prefix) = match entry.split_once('/') {
        Some((addr, prefix)) => (addr, Some(prefix)),
        None => (entry, None),
    };
    let net: IpAddr = addr.parse().ok()?;
    let max = if net.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix {
        Some(prefix) => prefix.parse::<u8>().ok().filter(|p| *p <= max)?,
        None => max,
    };
    Some(Cidr { net, prefix })
}

fn allowed_cidrs() -> &'static [Cidr] {
    static ALLOWED: OnceLock<Vec<Cidr>> = OnceLock::new();
    ALLOWED.get_or_init(|| {
        let raw = std::env::var("WEBCLAW_ALLOW_CIDRS").unwrap_or_default();
        raw.split(',')
            .filter(|entry| !entry.trim().is_empty())
            .filter_map(|entry| {
                let cidr = parse_cidr(entry);
                if cidr.is_none() {
                    tracing::warn!(
                        entry,
                        "WEBCLAW_ALLOW_CIDRS: ignoring an entry that is not an IP address or CIDR"
                    );
                }
                cidr
            })
            .collect()
    })
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
        || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0x0001)
        || (s[0] == 0x2001 && s[1] == 0x0000)
        || s[0] == 0x2002
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

    use super::{Cidr, is_blocked, is_blocked_ip, parse_cidr, validate_public_http_url};

    fn cidrs(list: &[&str]) -> Vec<Cidr> {
        list.iter().map(|s| parse_cidr(s).unwrap()).collect()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn parses_cidrs_and_bare_addresses() {
        for ok in [
            "198.18.0.0/15",
            " 10.1.2.3 ",
            "0.0.0.0/0",
            "fd00::/8",
            "2001:db8::1",
        ] {
            assert!(parse_cidr(ok).is_some(), "{ok}");
        }
        for bad in [
            "",
            "198.18.0.0/33",
            "fd00::/129",
            "nope",
            "1.2.3.4/x",
            "1.2.3/24",
        ] {
            assert!(parse_cidr(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn allowlist_lifts_the_block_for_its_ranges_only() {
        let allowed = cidrs(&["198.18.0.0/15"]);
        for open in ["198.18.0.1", "198.19.255.254", "::ffff:198.18.3.4"] {
            assert!(is_blocked(ip(open), &[]), "{open} blocked by default");
            assert!(!is_blocked(ip(open), &allowed), "{open}");
        }
        for closed in ["198.20.0.1", "10.0.0.1", "192.168.1.1", "100.64.0.1"] {
            assert_eq!(
                is_blocked(ip(closed), &allowed),
                is_blocked(ip(closed), &[]),
                "{closed}"
            );
        }
        assert!(!is_blocked(ip("8.8.8.8"), &allowed));
    }

    #[test]
    fn allowlist_never_opens_loopback_link_local_or_metadata() {
        let allowed = cidrs(&[
            "0.0.0.0/0",
            "::/0",
            "127.0.0.0/8",
            "169.254.0.0/16",
            "100.64.0.0/10",
            "fe80::/10",
            "fd00::/8",
            "64:ff9b::/96",
        ]);
        assert!(!is_blocked(ip("10.0.0.1"), &allowed));
        assert!(!is_blocked(ip("100.64.0.1"), &allowed));
        assert!(!is_blocked(ip("fd12::1"), &allowed));
        for closed in [
            "0.0.0.0",
            "127.0.0.1",
            "169.254.169.254",
            "169.254.0.1",
            "100.100.100.200",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fe80::1",
            "ff02::1",
            "fd00:ec2::254",
            "::ffff:169.254.169.254",
            "::ffff:127.0.0.1",
            "64:ff9b::a9fe:a9fe",
        ] {
            assert!(is_blocked(ip(closed), &allowed), "{closed}");
        }
    }

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
            Ipv4Addr::new(192, 0, 0, 8),
            Ipv4Addr::new(198, 18, 0, 1),
            Ipv4Addr::new(255, 255, 255, 255),
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
            "2002:7f00:1::".parse().unwrap(),
            "2002:a00:1::".parse().unwrap(),
            "2001:0:4136:e378:8000:63bf:3fff:fdd2".parse().unwrap(),
            "64:ff9b:1::a00:1".parse().unwrap(),
        ] {
            assert!(is_blocked_ip(IpAddr::V6(ip)), "{ip}");
        }
        for ip in [
            "2001:4860:4860::8888".parse::<Ipv6Addr>().unwrap(),
            "2606:4700:4700::1111".parse().unwrap(),
        ] {
            assert!(!is_blocked_ip(IpAddr::V6(ip)), "{ip}");
        }
    }

    #[tokio::test]
    async fn allows_public_ip_literals() {
        assert!(
            validate_public_http_url("https://93.184.216.34/")
                .await
                .is_ok()
        );
        assert!(!is_blocked_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    #[tokio::test]
    async fn blocks_localhost_domains_after_resolution() {
        assert!(validate_public_http_url("http://localhost/").await.is_err());
    }
}
