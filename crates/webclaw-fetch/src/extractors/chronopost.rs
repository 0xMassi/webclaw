//! Chronopost parcel-tracking structured extractor.
//!
//! The public tracking page (`/tracking-no-cms/suivi-page?listeNumerosLT=…`)
//! is an empty jQuery shell: `resources/js/trackinginfos.js` GETs
//! `/tracking-no-cms/suivi-colis` and injects the response, so fetching the
//! page itself yields a 200 with zero extractable words.
//!
//! That AJAX endpoint only answers with real data when the request carries
//! `X-Requested-With: XMLHttpRequest`. Without it Chronopost serves a
//! "Site en maintenance" page — a decoy that reads like a genuine outage and
//! will send you looking for a problem that isn't there.
//!
//! The endpoint returns JSON whose fields hold HTML fragments. Two matter:
//! - `top` — the milestone progress bar, one `<div id="stepN">` per
//!   milestone whose class carries the state (`before` = done, `active` =
//!   current, `after` = pending).
//! - `tab` — the scan-event table, one `<tr>` per event with `<br />`
//!   separating date/time and office/event-label.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "chronopost",
    label: "Chronopost tracking",
    description: "Returns parcel tracking: current status, milestone progress, and the full scan-event history.",
    url_patterns: &["https://www.chronopost.fr/tracking-no-cms/suivi-page?listeNumerosLT={number}"],
};

/// Host suffix check — Chronopost runs country sites but the tracking app
/// lives on the `.fr` domain, which is where the AJAX endpoint is rooted.
fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
}

pub fn matches(url: &str) -> bool {
    let host = host_of(url).to_ascii_lowercase();
    let host = host.split(':').next().unwrap_or("");
    if host != "chronopost.fr" && host != "www.chronopost.fr" {
        return false;
    }
    // The tracking number is the whole point — a bare /tracking-no-cms/ path
    // has nothing to extract.
    query_param(url, "listeNumerosLT").is_some_and(|v| !v.is_empty())
}

/// Pull a query parameter without pulling in a full URL parse for what is
/// always a flat `?a=b&c=d` string. Returns the percent-decoded value.
fn query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or("");
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

/// Minimal percent-decoder. Tracking numbers are alphanumeric and `langue`
/// is a two-letter code, so this only has to survive the occasional `%2C`
/// between numbers in a multi-parcel query.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let numbers = query_param(url, "listeNumerosLT")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            FetchError::Build(format!(
                "chronopost: no 'listeNumerosLT' tracking number in '{url}'"
            ))
        })?;
    // Chronopost accepts `en`/`fr`; anything else renders French. Default to
    // the caller's choice when present so labels come back in their language.
    let langue = query_param(url, "langue").unwrap_or_else(|| "en".to_string());

    let api_url = format!(
        "https://www.chronopost.fr/tracking-no-cms/suivi-colis?listeNumerosLT={numbers}&langue={langue}"
    );
    // `X-Requested-With` is load-bearing: without it the endpoint returns a
    // "Site en maintenance" HTML page instead of the tracking JSON. The
    // Referer mirrors what the real page sends.
    let resp = client
        .fetch_with_headers(
            &api_url,
            &[
                ("X-Requested-With", "XMLHttpRequest"),
                ("Referer", url),
                ("Accept", "text/html, */*; q=0.01"),
            ],
        )
        .await?;

    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "chronopost: tracking endpoint returned status {}",
            resp.status
        )));
    }

    let body = resp.html.trim_start();
    if body.starts_with('<') {
        // The maintenance decoy (or any other HTML) came back — almost always
        // means the AJAX header was dropped somewhere in the fetch path.
        return Err(FetchError::BodyDecode(
            "chronopost: endpoint returned HTML, not tracking JSON \
             (the 'Site en maintenance' decoy served when \
             'X-Requested-With: XMLHttpRequest' is missing)"
                .to_string(),
        ));
    }

    let payload: Value = serde_json::from_str(body)
        .map_err(|e| FetchError::BodyDecode(format!("chronopost: parse tracking JSON: {e}")))?;

    if let Some(err) = payload.get("error").and_then(Value::as_str)
        && !err.trim().is_empty()
    {
        return Err(FetchError::Build(format!(
            "chronopost: {}",
            strip_tags(err)
        )));
    }

    let top = payload.get("top").and_then(Value::as_str).unwrap_or("");
    let tab = payload.get("tab").and_then(Value::as_str).unwrap_or("");

    let steps = parse_steps(top);
    let status = steps
        .iter()
        .find(|s| s.state == "current")
        .map(|s| s.label.clone());
    let events = parse_events(tab);

    Ok(json!({
        "tracking_number": numbers,
        "language": langue,
        "status": status,
        "steps": steps
            .iter()
            .map(|s| json!({ "label": s.label, "state": s.state }))
            .collect::<Vec<_>>(),
        "events": events
            .iter()
            .map(|e| json!({
                "date": e.date,
                "time": e.time,
                "office": e.office,
                "event": e.event,
                "details": e.details,
            }))
            .collect::<Vec<_>>(),
        "event_count": events.len(),
        "source_url": url,
    }))
}

// -- parsing -----------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct Step {
    label: String,
    state: &'static str,
}

/// Parse the milestone progress bar out of the `top` fragment.
///
/// Chronopost renders the bar twice (desktop + mobile variants share the
/// same `id`s), so identical labels are de-duplicated, keeping first-seen
/// order.
fn parse_steps(top: &str) -> Vec<Step> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r#"(?s)<div[^>]*class="ch-suivi-colis-light-info([^"]*)".*?ch-suivi-colis-light-text"[^>]*>(.*?)</div>"#,
        )
        .expect("valid step regex")
    });

    let mut out: Vec<Step> = Vec::new();
    for cap in re.captures_iter(top) {
        let classes = cap.get(1).map_or("", |m| m.as_str());
        let label = strip_tags(cap.get(2).map_or("", |m| m.as_str()));
        if label.is_empty() {
            continue;
        }
        // `active` before `after`: the current step's class list is
        // `ch-suivi-colis-light-info active`, pending ones carry `after`.
        let state = if classes.contains("active") {
            "current"
        } else if classes.contains("after") {
            "pending"
        } else {
            "completed"
        };
        if !out.iter().any(|s| s.label == label) {
            out.push(Step { label, state });
        }
    }
    out
}

#[derive(Debug, PartialEq)]
struct Event {
    date: String,
    time: String,
    office: String,
    event: String,
    details: String,
}

/// Parse the scan-event table out of the `tab` fragment.
///
/// Each `<tr>` holds three cells: date/time, office/event label, and free-form
/// details — the first two split on `<br />`.
fn parse_events(tab: &str) -> Vec<Event> {
    static ROW: OnceLock<Regex> = OnceLock::new();
    static CELL: OnceLock<Regex> = OnceLock::new();
    let row_re =
        ROW.get_or_init(|| Regex::new(r"(?s)<tr[^>]*>(.*?)</tr>").expect("valid row regex"));
    let cell_re = CELL
        .get_or_init(|| Regex::new(r"(?s)<t[dh][^>]*>(.*?)</t[dh]>").expect("valid cell regex"));

    let mut out = Vec::new();
    for row in row_re.captures_iter(tab) {
        let inner = row.get(1).map_or("", |m| m.as_str());
        let cells: Vec<&str> = cell_re
            .captures_iter(inner)
            .filter_map(|c| c.get(1).map(|m| m.as_str()))
            .collect();
        if cells.len() < 2 {
            continue;
        }
        let (date, time) = split_br(cells[0]);
        let (office, event) = split_br(cells[1]);
        let details = cells.get(2).map(|c| strip_tags(c)).unwrap_or_default();
        // The header row has no date and no <br /> split — skip it.
        if date.is_empty() || time.is_empty() {
            continue;
        }
        out.push(Event {
            date,
            time,
            office,
            event,
            details,
        });
    }
    out
}

/// Split a cell on its `<br />` into (first, rest). A cell without a break
/// puts everything in the first slot.
fn split_br(cell: &str) -> (String, String) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)<br\s*/?>").expect("valid br regex"));
    let mut parts = re.splitn(cell, 2);
    let first = strip_tags(parts.next().unwrap_or(""));
    let rest = parts.next().map(strip_tags).unwrap_or_default();
    (first, rest)
}

/// Drop tags, decode the entities Chronopost actually emits, and collapse the
/// tab/newline runs its templates leave behind.
fn strip_tags(html: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?s)<[^>]+>").expect("valid tag regex"));
    let text = re.replace_all(html, " ");
    let decoded = text
        .replace("&nbsp;", " ")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        // `&amp;` last so a double-encoded `&amp;lt;` doesn't become a tag.
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_tracking_urls() {
        assert!(matches(
            "https://www.chronopost.fr/tracking-no-cms/suivi-page?listeNumerosLT=XU171986142JF&langue=en"
        ));
        assert!(matches(
            "https://chronopost.fr/tracking-no-cms/suivi-page?langue=fr&listeNumerosLT=XU171986142JF"
        ));
        // No tracking number => nothing to extract.
        assert!(!matches(
            "https://www.chronopost.fr/tracking-no-cms/suivi-page"
        ));
        assert!(!matches(
            "https://www.chronopost.fr/tracking-no-cms/suivi-page?listeNumerosLT="
        ));
        assert!(!matches("https://www.chronopost.fr/en"));
        // Host must not be spoofable by a lookalike or a userinfo prefix.
        assert!(!matches(
            "https://www.chronopost.fr.evil.com/x?listeNumerosLT=A1"
        ));
        assert!(!matches("https://user@evil.com/x?listeNumerosLT=A1"));
    }

    #[test]
    fn query_param_reads_values_and_decodes() {
        let url = "https://www.chronopost.fr/t?listeNumerosLT=AB1%2CCD2&langue=fr#frag";
        assert_eq!(
            query_param(url, "listeNumerosLT").as_deref(),
            Some("AB1,CD2")
        );
        assert_eq!(query_param(url, "langue").as_deref(), Some("fr"));
        assert_eq!(query_param(url, "missing"), None);
        assert_eq!(query_param("https://x.fr/no-query", "langue"), None);
    }

    #[test]
    fn parse_steps_marks_current_and_dedupes_variants() {
        // Shape lifted from the live `top` fragment, including the duplicated
        // desktop/mobile render.
        let top = r#"
            <div id="step1" class="ch-suivi-colis-light-info first before ">
              <div class="ch-suivi-colis-light-picto "></div>
              <div class="ch-suivi-colis-light-text">Under preparation at the shipper&#x27;s</div>
            </div>
            <div id="step5" class="ch-suivi-colis-light-info active ">
              <div class="ch-suivi-colis-light-picto"></div>
              <div class="ch-suivi-colis-light-text"> Delayed parcel </div>
            </div>
            <div id="step7" class="ch-suivi-colis-light-info after ">
              <div class="ch-suivi-colis-light-picto "></div>
              <div class="ch-suivi-colis-light-text">Out for delivery</div>
            </div>
            <div id="step5" class="ch-suivi-colis-light-info active ">
              <div class="ch-suivi-colis-light-picto"></div>
              <div class="ch-suivi-colis-light-text"> Delayed parcel </div>
            </div>"#;
        let steps = parse_steps(top);
        assert_eq!(steps.len(), 3, "duplicate mobile variant must be deduped");
        assert_eq!(steps[0].label, "Under preparation at the shipper's");
        assert_eq!(steps[0].state, "completed");
        assert_eq!(steps[1].label, "Delayed parcel");
        assert_eq!(steps[1].state, "current");
        assert_eq!(steps[2].state, "pending");
    }

    #[test]
    fn parse_events_splits_cells_and_skips_header() {
        // Real row shape, tabs and all.
        let tab = r#"<table><tr><th>Date and time</th><th>The steps of my delivery</th>
            <th>Supplement</th></tr>
            <tr class="toggleElmt">	<td>Wednesday 07/29/2026<br />08:39 AM</td>
            <td>CHRONOPOST NETWORKS<br />Parcel in transit</td>
            <td colspan="2">	Scan location : Vijfhuizen - NL (depot 0516)	<br />	</td></tr>
            <tr class="toggleElmt"><td>Friday 07/24/2026<br />03:49 PM</td>
            <td>Web Services<br />Shipment in preparation</td>
            <td colspan="2">Partner number : GEO/042&amp;33</td></tr></table>"#;
        let events = parse_events(tab);
        assert_eq!(events.len(), 2, "header row must be skipped");
        assert_eq!(events[0].date, "Wednesday 07/29/2026");
        assert_eq!(events[0].time, "08:39 AM");
        assert_eq!(events[0].office, "CHRONOPOST NETWORKS");
        assert_eq!(events[0].event, "Parcel in transit");
        assert_eq!(
            events[0].details,
            "Scan location : Vijfhuizen - NL (depot 0516)"
        );
        assert_eq!(events[1].details, "Partner number : GEO/042&33");
    }

    #[test]
    fn strip_tags_decodes_entities_and_collapses_whitespace() {
        assert_eq!(
            strip_tags("<td>\t\tUnder preparation at the shipper&#x27;s\n</td>"),
            "Under preparation at the shipper's"
        );
        assert_eq!(strip_tags("a&nbsp;&nbsp;b"), "a b");
        assert_eq!(strip_tags("x &amp; y &gt; z"), "x & y > z");
    }
}
