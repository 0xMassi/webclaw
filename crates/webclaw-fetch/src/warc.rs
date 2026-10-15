//! Web archive files (WARC 1.1, ISO 28500) of the HTTP responses a
//! [`FetchClient`](crate::FetchClient) receives. Set
//! [`FetchConfig::archive`](crate::FetchConfig::archive) to a callback that
//! hands each [`Exchange`] to a [`Writer`].

use std::io::{self, Write};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// One HTTP response and the request that asked for it, as the client
/// received it, with the body decoded when the server compressed it: the
/// HTTP client decodes it before webclaw sees it.
#[derive(Debug, Clone, Copy)]
pub struct Exchange<'a> {
    /// The URL that answered.
    pub url: &'a str,
    /// The headers sent with the request: the browser profile's, the
    /// configured ones and the request's own. Cookies the client stored from
    /// earlier responses are not listed.
    pub request_headers: &'a http::HeaderMap,
    pub status: u16,
    pub version: http::Version,
    /// The response headers, without `Content-Encoding` and `Content-Length`
    /// when the body was decoded.
    pub headers: &'a http::HeaderMap,
    pub body: &'a [u8],
    /// The address of the server or proxy that answered.
    pub ip: Option<std::net::IpAddr>,
}

/// A callback that receives every HTTP response a client gets; see
/// [`FetchConfig::archive`](crate::FetchConfig::archive).
#[derive(Clone)]
pub struct Archive(pub Arc<dyn Fn(&Exchange<'_>) + Send + Sync>);

impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Archive")
    }
}

/// Writes a WARC file: a `warcinfo` record, then a `response` and a
/// `request` record for each [`Exchange`], each pointing at the `warcinfo`
/// with `WARC-Warcinfo-ID`. With `gzip`, every record is its own gzip
/// member, as `.warc.gz` files hold them, so a file cut short keeps every
/// record before the cut readable.
///
/// Responses are stored decoded, which the `warcinfo` record states: a
/// compressed body is stored decompressed, without its `Content-Encoding`,
/// `Content-Length` and `Transfer-Encoding` headers, and HTTP/2 responses
/// get an HTTP/1.1 status line. Payload digests are therefore those of the
/// decoded body, and do not match a copy of the same resource that another
/// archiver stored still compressed.
pub struct Writer<W: Write> {
    out: W,
    gzip: bool,
    info: String,
}

impl<W: Write> Writer<W> {
    /// Start a WARC file in `out` with a `warcinfo` record naming `software`.
    pub fn new(out: W, gzip: bool, software: &str) -> io::Result<Self> {
        let mut writer = Self {
            out,
            gzip,
            info: record_id(),
        };
        let info = format!(
            "software: {software}\r\nformat: WARC File Format 1.1\r\nconformsTo: http://iipc.github.io/warc-specifications/specifications/warc-format/warc-1.1/\r\ndescription: HTTP responses are stored decoded: compressed bodies decompressed, without their Content-Encoding, Content-Length and Transfer-Encoding headers, and HTTP/2 responses with an HTTP/1.1 status line\r\n"
        );
        writer.record(
            &[
                ("WARC-Type", "warcinfo".into()),
                ("WARC-Record-ID", writer.info.clone()),
                ("WARC-Date", warc_date(SystemTime::now())),
                ("Content-Type", "application/warc-fields".into()),
            ],
            info.as_bytes(),
        )?;
        writer.out.flush()?;
        Ok(writer)
    }

    /// Append the request and response records of `exchange` and flush them.
    pub fn write(&mut self, exchange: &Exchange<'_>) -> io::Result<()> {
        let url = url::Url::parse(exchange.url)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let date = warc_date(SystemTime::now());
        let response_id = record_id();
        let mut target = url.path().to_string();
        if let Some(query) = url.query() {
            target.push('?');
            target.push_str(query);
        }
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
            None => url.host_str().unwrap_or_default().to_string(),
        };
        let mut request = format!("GET {target} HTTP/1.1\r\nhost: {host}\r\n").into_bytes();
        for (name, value) in exchange.request_headers {
            if name != http::header::HOST {
                header_line(&mut request, name, value);
            }
        }
        request.extend_from_slice(b"\r\n");

        let status = http::StatusCode::from_u16(exchange.status).ok();
        let version = match exchange.version {
            http::Version::HTTP_10 => "HTTP/1.0",
            _ => "HTTP/1.1",
        };
        let mut response = format!(
            "{version} {} {}\r\n",
            exchange.status,
            status.and_then(|s| s.canonical_reason()).unwrap_or("")
        )
        .into_bytes();
        for (name, value) in exchange.headers {
            if name != http::header::TRANSFER_ENCODING {
                header_line(&mut response, name, value);
            }
        }
        response.extend_from_slice(b"\r\n");
        let head = response.len();
        response.extend_from_slice(exchange.body);

        let mut fields = vec![
            ("WARC-Type", "response".to_string()),
            ("WARC-Record-ID", response_id.clone()),
            ("WARC-Date", date.clone()),
            ("WARC-Target-URI", exchange.url.to_string()),
            ("WARC-Warcinfo-ID", self.info.clone()),
        ];
        if let Some(ip) = exchange.ip {
            fields.push(("WARC-IP-Address", ip.to_string()));
        }
        fields.push(("WARC-Payload-Digest", digest(&response[head..])));
        fields.push((
            "Content-Type",
            "application/http; msgtype=response".to_string(),
        ));
        self.record(&fields, &response)?;
        self.record(
            &[
                ("WARC-Type", "request".into()),
                ("WARC-Record-ID", record_id()),
                ("WARC-Date", date),
                ("WARC-Target-URI", exchange.url.to_string()),
                ("WARC-Concurrent-To", response_id),
                ("WARC-Warcinfo-ID", self.info.clone()),
                ("Content-Type", "application/http; msgtype=request".into()),
            ],
            &request,
        )?;
        self.out.flush()
    }

    /// The writer the file went to.
    pub fn into_inner(self) -> W {
        self.out
    }

    fn record(&mut self, fields: &[(&str, String)], block: &[u8]) -> io::Result<()> {
        let mut record = b"WARC/1.1\r\n".to_vec();
        for (name, value) in fields {
            record.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        record.extend_from_slice(
            format!(
                "WARC-Block-Digest: {}\r\nContent-Length: {}\r\n\r\n",
                digest(block),
                block.len()
            )
            .as_bytes(),
        );
        record.extend_from_slice(block);
        record.extend_from_slice(b"\r\n\r\n");
        if self.gzip {
            let mut gz =
                flate2::write::GzEncoder::new(&mut self.out, flate2::Compression::default());
            gz.write_all(&record)?;
            gz.finish()?;
            Ok(())
        } else {
            self.out.write_all(&record)
        }
    }
}

fn header_line(out: &mut Vec<u8>, name: &http::HeaderName, value: &http::HeaderValue) {
    out.extend_from_slice(name.as_str().as_bytes());
    out.extend_from_slice(b": ");
    out.extend_from_slice(value.as_bytes());
    out.extend_from_slice(b"\r\n");
}

fn record_id() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "<urn:uuid:{}-{}-{}-{}-{}>",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

fn warc_date(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn digest(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let hash = sha1(data);
    let mut out = String::from("sha1:");
    for chunk in hash.chunks(5) {
        let n = chunk.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
        for i in (0..8).rev() {
            out.push(ALPHABET[(n >> (5 * i) & 31) as usize] as char);
        }
    }
    out
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..20 => ((b & c) | (!b & d), 0x5A827999),
                20..40 => (b ^ c ^ d, 0x6ED9EBA1),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (h, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *h = h.wrapping_add(v);
        }
    }
    let mut out = [0u8; 20];
    for (chunk, word) in out.chunks_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha1_matches_the_standard_test_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(
            hex(&sha1(&vec![b'a'; 1_000_000])),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
        assert_eq!(digest(b""), "sha1:3I42H3S6NNFQ2MSVX7XZKYAYSCX5QBYJ");
    }

    #[test]
    fn dates_are_utc_in_iso_8601() {
        assert_eq!(warc_date(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let leap = UNIX_EPOCH + std::time::Duration::from_secs(951_782_400 + 3_723);
        assert_eq!(warc_date(leap), "2000-02-29T01:02:03Z");
        let late = UNIX_EPOCH + std::time::Duration::from_secs(4_102_444_799);
        assert_eq!(warc_date(late), "2099-12-31T23:59:59Z");
    }

    type Record = (Vec<(String, String)>, Vec<u8>);

    fn records(file: &[u8]) -> Vec<Record> {
        let mut rest = file;
        let mut out = Vec::new();
        while !rest.is_empty() {
            let end = rest.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let head = std::str::from_utf8(&rest[..end]).unwrap();
            let mut lines = head.split("\r\n");
            assert_eq!(lines.next(), Some("WARC/1.1"));
            let fields: Vec<(String, String)> = lines
                .map(|l| {
                    let (k, v) = l.split_once(": ").unwrap();
                    (k.to_string(), v.to_string())
                })
                .collect();
            let len: usize = fields
                .iter()
                .find(|(k, _)| k == "Content-Length")
                .unwrap()
                .1
                .parse()
                .unwrap();
            let block = rest[end + 4..end + 4 + len].to_vec();
            assert_eq!(&rest[end + 4 + len..end + 8 + len], b"\r\n\r\n");
            rest = &rest[end + 8 + len..];
            out.push((fields, block));
        }
        out
    }

    fn field<'a>(fields: &'a [(String, String)], name: &str) -> &'a str {
        &fields.iter().find(|(k, _)| k == name).unwrap().1
    }

    #[test]
    fn an_exchange_becomes_a_response_and_a_request_record_with_their_digests() {
        let mut sent = http::HeaderMap::new();
        sent.insert("user-agent", "test".parse().unwrap());
        let mut got = http::HeaderMap::new();
        got.insert("content-type", "text/html; charset=utf-8".parse().unwrap());
        got.insert("transfer-encoding", "chunked".parse().unwrap());
        let exchange = Exchange {
            url: "https://example.com:8443/a?b=1",
            request_headers: &sent,
            status: 404,
            version: http::Version::HTTP_2,
            headers: &got,
            body: b"<p>gone</p>",
            ip: Some("93.184.215.14".parse().unwrap()),
        };
        let mut writer = Writer::new(Vec::new(), false, "webclaw test").unwrap();
        writer.write(&exchange).unwrap();
        let file = writer.into_inner();
        let records = records(&file);
        assert_eq!(records.len(), 3);
        for (fields, block) in &records {
            assert_eq!(field(fields, "WARC-Block-Digest"), digest(block));
        }
        let (info, block) = &records[0];
        assert_eq!(field(info, "WARC-Type"), "warcinfo");
        assert!(String::from_utf8_lossy(block).contains("software: webclaw test"));
        assert!(
            String::from_utf8_lossy(block)
                .contains("description: HTTP responses are stored decoded"),
            "{}",
            String::from_utf8_lossy(block)
        );
        for (fields, _) in &records[1..] {
            assert_eq!(
                field(fields, "WARC-Warcinfo-ID"),
                field(info, "WARC-Record-ID")
            );
        }
        let (response, block) = &records[1];
        assert_eq!(field(response, "WARC-Type"), "response");
        assert_eq!(
            field(response, "WARC-Target-URI"),
            "https://example.com:8443/a?b=1"
        );
        assert_eq!(field(response, "WARC-IP-Address"), "93.184.215.14");
        assert_eq!(
            field(response, "WARC-Payload-Digest"),
            digest(b"<p>gone</p>")
        );
        assert_eq!(
            String::from_utf8_lossy(block),
            "HTTP/1.1 404 Not Found\r\ncontent-type: text/html; charset=utf-8\r\n\r\n<p>gone</p>"
        );
        let (request, block) = &records[2];
        assert_eq!(field(request, "WARC-Type"), "request");
        assert_eq!(
            field(request, "WARC-Concurrent-To"),
            field(response, "WARC-Record-ID")
        );
        assert_eq!(
            String::from_utf8_lossy(block),
            "GET /a?b=1 HTTP/1.1\r\nhost: example.com:8443\r\nuser-agent: test\r\n\r\n"
        );
        let ids: std::collections::HashSet<_> = records
            .iter()
            .map(|(f, _)| field(f, "WARC-Record-ID"))
            .collect();
        assert_eq!(ids.len(), 3);
        assert!(field(info, "WARC-Record-ID").starts_with("<urn:uuid:"));
    }

    #[test]
    fn gzip_puts_every_record_in_its_own_member() {
        let headers = http::HeaderMap::new();
        let exchange = Exchange {
            url: "https://example.com/",
            request_headers: &headers,
            status: 200,
            version: http::Version::HTTP_11,
            headers: &headers,
            body: b"hi",
            ip: None,
        };
        let mut writer = Writer::new(Vec::new(), true, "webclaw test").unwrap();
        writer.write(&exchange).unwrap();
        let file = writer.into_inner();
        let mut members = 0;
        let mut plain = Vec::new();
        let mut rest = &file[..];
        while !rest.is_empty() {
            let mut gz = flate2::bufread::GzDecoder::new(rest);
            std::io::Read::read_to_end(&mut gz, &mut plain).unwrap();
            rest = gz.into_inner();
            members += 1;
        }
        assert_eq!(members, 3);
        assert_eq!(records(&plain).len(), 3);
    }
}
