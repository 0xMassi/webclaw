//! Stack Overflow Q&A structured extractor.
//!
//! Uses the Stack Exchange API at `api.stackexchange.com/2.3/questions/{id}`
//! with `site=stackoverflow`. Two calls: one for the question, one for
//! its answers. Both come pre-filtered to include the rendered HTML body
//! so we don't re-parse the question page itself.
//!
//! Anonymous access caps at 300 requests per IP per day. Production
//! cloud should set `STACKAPPS_KEY` to lift to 10,000/day, but we don't
//! require it to work out of the box.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "stackoverflow",
    label: "Stack Overflow Q&A",
    description: "Returns question + answers: title, body, tags, votes, accepted answer, top answers.",
    url_patterns: &["https://stackoverflow.com/questions/{id}/{slug}"],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host != "stackoverflow.com" && host != "www.stackoverflow.com" {
        return false;
    }
    parse_question_id(url).is_some()
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let id = parse_question_id(url).ok_or_else(|| {
        FetchError::Build(format!(
            "stackoverflow: cannot parse question id from '{url}'"
        ))
    })?;

    // Filter `withbody` includes the rendered HTML body for both questions
    // and answers. Stack Exchange's filter system is documented at
    // api.stackexchange.com/docs/filters.
    let q_url = format!(
        "https://api.stackexchange.com/2.3/questions/{id}?site=stackoverflow&filter=withbody"
    );
    let q_resp = client.fetch(&q_url).await?;
    if q_resp.status != 200 {
        return Err(FetchError::Build(format!(
            "stackexchange api returned status {}",
            q_resp.status
        )));
    }
    let q_body: QResponse = serde_json::from_str(&q_resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("stackoverflow q parse: {e}")))?;
    let q = q_body
        .items
        .first()
        .ok_or_else(|| FetchError::Build(format!("stackoverflow: question {id} not found")))?;

    let a_url = format!(
        "https://api.stackexchange.com/2.3/questions/{id}/answers?site=stackoverflow&filter=withbody&order=desc&sort=votes"
    );
    let a_resp = client.fetch(&a_url).await?;
    let answers = if a_resp.status == 200 {
        let a_body: AResponse = serde_json::from_str(&a_resp.html)
            .map_err(|e| FetchError::BodyDecode(format!("stackoverflow a parse: {e}")))?;
        a_body
            .items
            .iter()
            .map(|a| {
                json!({
                    "answer_id":     a.answer_id,
                    "is_accepted":   a.is_accepted,
                    "score":         a.score,
                    "body":          a.body,
                    "creation_date": a.creation_date,
                    "last_edit_date":a.last_edit_date,
                    "author":        a.owner.as_ref().and_then(|o| o.display_name.clone()),
                    "author_rep":    a.owner.as_ref().and_then(|o| o.reputation),
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let accepted = answers
        .iter()
        .find(|a| {
            a.get("is_accepted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .cloned();

    Ok(json!({
        "url":            url,
        "question_id":    q.question_id,
        "title":          q.title,
        "body":           q.body,
        "tags":           q.tags,
        "score":          q.score,
        "view_count":     q.view_count,
        "answer_count":   q.answer_count,
        "is_answered":    q.is_answered,
        "accepted_answer_id": q.accepted_answer_id,
        "creation_date":  q.creation_date,
        "last_activity_date": q.last_activity_date,
        "author":         q.owner.as_ref().and_then(|o| o.display_name.clone()),
        "author_rep":     q.owner.as_ref().and_then(|o| o.reputation),
        "link":           q.link,
        "accepted_answer": accepted,
        "top_answers":    answers,
    }))
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

/// Parse question id from a URL of the form `/questions/{id}/{slug}`.
fn parse_question_id(url: &str) -> Option<u64> {
    let after = url.split("/questions/").nth(1)?;
    let stripped = after.split(['?', '#']).next()?.trim_end_matches('/');
    let first = stripped.split('/').next()?;
    first.parse::<u64>().ok()
}

// ---------------------------------------------------------------------------
// Stack Exchange API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct QResponse {
    #[serde(default)]
    items: Vec<Question>,
}

#[derive(Deserialize)]
struct Question {
    question_id: Option<u64>,
    title: Option<String>,
    body: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    score: Option<i64>,
    view_count: Option<i64>,
    answer_count: Option<i64>,
    is_answered: Option<bool>,
    accepted_answer_id: Option<u64>,
    creation_date: Option<i64>,
    last_activity_date: Option<i64>,
    owner: Option<Owner>,
    link: Option<String>,
}

#[derive(Deserialize)]
struct AResponse {
    #[serde(default)]
    items: Vec<Answer>,
}

#[derive(Deserialize)]
struct Answer {
    answer_id: Option<u64>,
    is_accepted: Option<bool>,
    score: Option<i64>,
    body: Option<String>,
    creation_date: Option<i64>,
    last_edit_date: Option<i64>,
    owner: Option<Owner>,
}

#[derive(Deserialize)]
struct Owner {
    display_name: Option<String>,
    reputation: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_question_urls() {
        assert!(matches(
            "https://stackoverflow.com/questions/12345/some-slug"
        ));
        assert!(matches(
            "https://stackoverflow.com/questions/12345/some-slug?answertab=votes"
        ));
        assert!(!matches("https://stackoverflow.com/"));
        assert!(!matches("https://stackoverflow.com/questions"));
        assert!(!matches("https://stackoverflow.com/users/100"));
        assert!(!matches("https://example.com/questions/12345/x"));
    }

    #[test]
    fn parse_question_id_handles_slug_and_query() {
        assert_eq!(
            parse_question_id("https://stackoverflow.com/questions/12345/some-slug"),
            Some(12345)
        );
        assert_eq!(
            parse_question_id("https://stackoverflow.com/questions/12345/some-slug?tab=newest"),
            Some(12345)
        );
        assert_eq!(parse_question_id("https://stackoverflow.com/foo"), None);
    }
}
