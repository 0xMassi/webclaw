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

pub fn matches(url: &str) -> bool {
    parse_tracking(url).is_some()
}

/// What a tracking URL yields once validated.
struct Tracking {
    /// `listeNumerosLT`, percent-decoded. May be a comma-separated list.
    numbers: String,
    /// `langue`, defaulting to `en`.
    langue: String,
    /// The caller's URL with any credentials removed, safe to echo back as
    /// a `Referer` header.
    referer: String,
}

/// Parse a tracking URL into `(tracking numbers, language)`.
///
/// Returns `None` unless the host is exactly `chronopost.fr` /
/// `www.chronopost.fr` and a non-empty `listeNumerosLT` is present — the
/// tracking number is the whole point, and this predicate gates
/// `dispatch_by_url` auto-detect, so it must not claim a URL it can't serve.
///
/// Uses `Url` rather than hand-rolled string splitting. That is not just
/// tidiness: splitting on `://`, `/`, then `@` reads the host out of the
/// *query* on a URL with no path (`https://evil.com?x&@www.chronopost.fr`),
/// and a hand-rolled percent-decoder that slices `&s[i+1..i+3]` panics when
/// a `%` is followed by a multi-byte char. `Url` also normalises case,
/// punycodes IDN, strips userinfo and port from `host_str`, and decodes
/// query values without ever slicing off a char boundary.
fn parse_tracking(url: &str) -> Option<Tracking> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host != "chronopost.fr" && host != "www.chronopost.fr" {
        return None;
    }

    let mut numbers = None;
    let mut langue = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "listeNumerosLT" if !v.is_empty() => numbers = Some(v.into_owned()),
            "langue" if !v.is_empty() => langue = Some(v.into_owned()),
            _ => {}
        }
    }

    // `https://user:pw@www.chronopost.fr/?...` is a legitimate URL, but the
    // raw input is echoed back as `Referer` — so drop the credentials rather
    // than forwarding them to Chronopost.
    let mut referer = parsed.clone();
    let _ = referer.set_username("");
    let _ = referer.set_password(None);

    Some(Tracking {
        numbers: numbers?,
        // Chronopost accepts `en`/`fr`; anything else renders French. Default
        // to English so labels are readable when the caller didn't ask.
        langue: langue.unwrap_or_else(|| "en".to_string()),
        referer: referer.to_string(),
    })
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let Tracking {
        numbers,
        langue,
        referer,
    } = parse_tracking(url).ok_or_else(|| {
        FetchError::Build(format!(
            "chronopost: no 'listeNumerosLT' tracking number in '{url}'"
        ))
    })?;

    // Built through `query_pairs_mut` rather than `format!`: the values come
    // back percent-DECODED, so interpolating them raw would let a number like
    // `X%26langue%3Dfr` inject an extra parameter into our own request.
    let mut api = url::Url::parse("https://www.chronopost.fr/tracking-no-cms/suivi-colis")
        .expect("static URL is valid");
    api.query_pairs_mut()
        .append_pair("listeNumerosLT", &numbers)
        .append_pair("langue", &langue);
    let api_url = api.to_string();
    // `X-Requested-With` is load-bearing: without it the endpoint returns a
    // "Site en maintenance" HTML page instead of the tracking JSON. The
    // Referer mirrors what the real page sends.
    let resp = client
        .fetch_with_headers(
            &api_url,
            &[
                ("X-Requested-With", "XMLHttpRequest"),
                ("Referer", referer.as_str()),
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

    // Fail loudly rather than returning a well-formed empty result. The
    // endpoint answered 200 with JSON, so if we recovered neither a milestone
    // nor a scan event the markup has moved and our selectors are stale —
    // reporting `Ok` with empty arrays would make a site redesign look like a
    // parcel with no history, and (via `dispatch_by_url`) would stop the
    // caller from falling back to the generic scrape path.
    if steps.is_empty() && events.is_empty() {
        // Distinguish "this number has no data" from "our selectors broke".
        // Both produce an empty parse, and structure can't tell them apart —
        // the `ch-colis-information` block wraps the not-found message in the
        // empty case AND the summary in the normal one. Chronopost does say
        // so in prose, and it only renders `en` or `fr`, so match both. If
        // neither matches, the markup really has moved: keep that alarm loud
        // and distinct, because a routine bad number must not trip it.
        // Matched on apostrophe-FREE fragments: the message contains one, and
        // whether it arrives as `&#x27;`, `&rsquo;` or a literal U+2019 is a
        // template detail. Anchoring around it keeps a routine bad number from
        // re-tripping the markup-moved alarm on a typographic-quote change.
        let top_text = strip_tags(top).to_lowercase();
        let unknown_parcel =
            top_text.contains("have any information") || top_text.contains("avons pas d");
        return Err(if unknown_parcel {
            FetchError::Build(format!(
                "chronopost: no tracking information for '{numbers}' — check the number"
            ))
        } else {
            FetchError::BodyDecode(
                "chronopost: tracking response contained neither milestones nor \
                 scan events — the page markup has likely changed"
                    .to_string(),
            )
        });
    }

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
///
/// Each milestone is sliced out on its own boundary before the label is read,
/// rather than matched with one pattern spanning both `<div>`s. A single
/// spanning pattern bridges two milestones whenever one renders without a
/// text child — it would pair step N's class list with step N+1's label,
/// mis-assigning the state and silently swallowing a step. (`regex` has no
/// lookaround, so the gap can't simply be told to stop at the next
/// milestone.) `[^"]*` after each class name tolerates the trailing space
/// Chronopost's templates emit, e.g. `class="ch-suivi-colis-light-text "`.
fn parse_steps(top: &str) -> Vec<Step> {
    static INFO: OnceLock<Regex> = OnceLock::new();
    static TEXT: OnceLock<Regex> = OnceLock::new();
    let info_re = INFO.get_or_init(|| {
        // `(?:\s[^"]*)?` — the class name must end at the quote or at
        // whitespace, so this can't also claim `…-light-infobox`.
        Regex::new(r#"<div[^>]*class="ch-suivi-colis-light-info(\s[^"]*)?"[^>]*>"#)
            .expect("valid step-info regex")
    });
    let text_re = TEXT.get_or_init(|| {
        // `(?:\s[^"]*)?` — the name must end at the quote or at whitespace,
        // which rejects `…-light-textual` while still tolerating both the
        // trailing space Chronopost emits and any additional modifier class.
        // Non-capturing so group 1 stays the label.
        Regex::new(r#"(?s)class="ch-suivi-colis-light-text(?:\s[^"]*)?"[^>]*>(.*?)</div>"#)
            .expect("valid step-text regex")
    });

    // (class list, where this milestone's content starts, where its opening
    // tag starts). Byte offsets from `regex` are always char boundaries.
    let heads: Vec<(&str, usize, usize)> = info_re
        .captures_iter(top)
        .filter_map(|c| {
            let whole = c.get(0)?;
            // Group 1 is optional — a milestone whose class list is exactly
            // `ch-suivi-colis-light-info` has no trailing classes and must
            // still count (it just has no state keyword, i.e. "completed").
            let classes = c.get(1).map_or("", |m| m.as_str());
            Some((classes, whole.end(), whole.start()))
        })
        .collect();

    let mut out: Vec<Step> = Vec::new();
    for (i, (classes, content_start, _)) in heads.iter().enumerate() {
        // Stop at the next milestone's opening tag so a label can never be
        // borrowed from the following step.
        let stop = heads
            .get(i + 1)
            .map(|(_, _, next_start)| *next_start)
            .unwrap_or(top.len());
        if stop <= *content_start {
            continue;
        }
        let Some(label) = text_re
            .captures(&top[*content_start..stop])
            .and_then(|c| c.get(1).map(|m| strip_tags(m.as_str())))
        else {
            continue;
        };
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
        // Typographic apostrophe, in all three spellings plus the literal
        // char. French copy uses it freely and it shows up inside labels
        // ("shipper's"), so fold it to ASCII rather than leaving a raw entity.
        .replace("&rsquo;", "'")
        .replace("&#8217;", "'")
        .replace("&#x2019;", "'")
        .replace('\u{2019}', "'")
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
        assert!(!matches(
            "https://chronopost.fr@evil.com/x?listeNumerosLT=A1"
        ));
        // Regression: with no path segment there is no `/` to split on, so
        // hand-rolled host parsing read the host out of the QUERY and matched.
        assert!(!matches(
            "https://evil.com?listeNumerosLT=A1&@www.chronopost.fr"
        ));
        assert!(!matches(
            r"https://evil.com\@www.chronopost.fr/?listeNumerosLT=A1"
        ));
        // Not a URL at all must be rejected, not panic.
        assert!(!matches("not a url"));
        assert!(!matches(""));
    }

    #[test]
    fn parse_tracking_decodes_and_defaults_language() {
        let t =
            parse_tracking("https://www.chronopost.fr/t?listeNumerosLT=AB1%2CCD2&langue=fr#frag")
                .expect("should parse");
        assert_eq!(t.numbers, "AB1,CD2");
        assert_eq!(t.langue, "fr");

        // `langue` absent => English default.
        let t = parse_tracking("https://www.chronopost.fr/t?listeNumerosLT=AB1").expect("parses");
        assert_eq!(t.langue, "en");

        assert!(parse_tracking("https://www.chronopost.fr/no-query").is_none());
    }

    #[test]
    fn parse_tracking_strips_credentials_from_referer() {
        // The caller's URL is echoed back as `Referer`; credentials in it
        // must not be forwarded to Chronopost.
        let t = parse_tracking("https://user:hunter2@www.chronopost.fr/t?listeNumerosLT=AB1")
            .expect("credentials are legal in a URL, so this still parses");
        assert!(
            !t.referer.contains("hunter2") && !t.referer.contains("user"),
            "referer must not carry credentials, got {}",
            t.referer
        );
    }

    #[test]
    fn parse_tracking_rejects_non_http_schemes() {
        assert!(parse_tracking("ftp://www.chronopost.fr/?listeNumerosLT=A1").is_none());
        assert!(parse_tracking("wacky://www.chronopost.fr/?listeNumerosLT=A1").is_none());
        assert!(parse_tracking("http://www.chronopost.fr/?listeNumerosLT=A1").is_some());
    }

    #[test]
    fn parse_tracking_survives_malformed_percent_escapes() {
        // Regression: a hand-rolled decoder sliced `&s[i+1..i+3]` on byte
        // indices and panicked when a `%` was followed by a multi-byte char.
        // This ran inside `matches()`, i.e. before any fetch, so it was
        // reachable from every dispatch path.
        for bad in [
            "https://www.chronopost.fr/t?listeNumerosLT=%a\u{e9}",
            "https://www.chronopost.fr/t?listeNumerosLT=%",
            "https://www.chronopost.fr/t?listeNumerosLT=%zz",
            "https://www.chronopost.fr/t?listeNumerosLT=\u{e9}%",
        ] {
            // Must not panic; value is whatever the URL parser makes of it.
            let _ = matches(bad);
            let _ = parse_tracking(bad);
        }
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
    fn parse_steps_does_not_bridge_a_step_with_no_label() {
        // Regression: one pattern spanning both <div>s paired step1's class
        // list with step2's label, so the state was wrong AND a step vanished.
        let top = r#"
            <div id="step1" class="ch-suivi-colis-light-info first before ">
              <div class="ch-suivi-colis-light-picto "></div>
            </div>
            <div id="step2" class="ch-suivi-colis-light-info active ">
              <div class="ch-suivi-colis-light-picto"></div>
              <div class="ch-suivi-colis-light-text">Delivered</div>
            </div>"#;
        let steps = parse_steps(top);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].label, "Delivered");
        assert_eq!(
            steps[0].state, "current",
            "label must keep its OWN step's state, not the previous step's"
        );
    }

    #[test]
    fn parse_steps_ignores_classes_that_merely_share_a_prefix() {
        // `[^"]*` after the class name also matched `…-light-infobox` and
        // `…-light-textual`, inventing a step out of unrelated markup.
        let top = r#"<div class="ch-suivi-colis-light-infobox foo">
              <div class="ch-suivi-colis-light-textual">Not a step</div>
            </div>"#;
        assert!(parse_steps(top).is_empty());
    }

    #[test]
    fn parse_steps_tolerates_a_modifier_class_on_the_label() {
        // Tightening the text class to `\s*` required it to be the ONLY
        // class, silently dropping a step whose label carried a modifier.
        let top = r#"<div class="ch-suivi-colis-light-info active ">
              <div class="ch-suivi-colis-light-text bold">Delivered</div>
            </div>"#;
        let steps = parse_steps(top);
        assert_eq!(steps.len(), 1, "a modifier class must not drop the step");
        assert_eq!(steps[0].label, "Delivered");
        assert_eq!(steps[0].state, "current");
    }

    #[test]
    fn strip_tags_folds_typographic_apostrophes() {
        // The unknown-parcel prose match and the milestone labels both depend
        // on this; French copy uses U+2019 freely.
        for raw in [
            "shipper&#x27;s",
            "shipper&rsquo;s",
            "shipper&#8217;s",
            "shipper&#x2019;s",
            "shipper\u{2019}s",
        ] {
            assert_eq!(strip_tags(raw), "shipper's", "failed for {raw}");
        }
    }

    #[test]
    fn parse_steps_keeps_a_step_with_no_extra_classes() {
        // The state keyword is optional; a bare class list means "completed".
        let top = r#"<div class="ch-suivi-colis-light-info">
              <div class="ch-suivi-colis-light-text">Taken up</div>
            </div>"#;
        let steps = parse_steps(top);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].state, "completed");
    }

    #[test]
    fn parse_steps_tolerates_trailing_space_in_class() {
        // Chronopost's templates emit `class="ch-suivi-colis-light-picto "`,
        // so the text class can carry a trailing space too. Requiring the
        // quote immediately after the name returned zero steps, silently.
        let top = r#"<div class="ch-suivi-colis-light-info active ">
              <div class="ch-suivi-colis-light-text ">Out for delivery</div>
            </div>"#;
        let steps = parse_steps(top);
        assert_eq!(steps.len(), 1, "trailing space must not drop the step");
        assert_eq!(steps[0].label, "Out for delivery");
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
