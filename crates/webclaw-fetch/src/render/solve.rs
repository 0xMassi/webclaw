//! Getting a rendered page past a Cloudflare challenge.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::dom::{GetBoxModelParams, Node};
use chromiumoxide::layout::Point;
use rand::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Challenge {
    Wait,
    Checkbox,
    Widget,
}

const WIDGET_PATIENCE: Duration = Duration::from_secs(10);
const WIDGET_SHOWS_WITHIN: Duration = Duration::from_secs(3);
const SHOWN_FOR: Duration = Duration::from_millis(1200);
const RECLICK_AFTER: Duration = Duration::from_secs(6);

fn challenge(html: &str) -> Option<Challenge> {
    if crate::cloud::is_cloudflare_challenge(html, &http::HeaderMap::new()) {
        return Some(if html.contains("cType: 'non-interactive'") {
            Challenge::Wait
        } else {
            Challenge::Checkbox
        });
    }
    (WIDGET.is_match(html) && !has_token(html)).then_some(Challenge::Widget)
}

static WIDGET: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?is)<script\b[^>]*\bsrc\s*=\s*["']?[^"'>\s]*challenges\.cloudflare\.com/turnstile/|<[a-z][a-z0-9-]*\b[^>]*\bclass\s*=\s*["'](?:[^"']*\s)?cf-turnstile(?:\s[^"']*)?["']"#,
    )
    .expect("a valid pattern")
});

fn has_token(html: &str) -> bool {
    ["\"cf-turnstile-response\"", "\"g-recaptcha-response\""]
        .iter()
        .any(|name| {
            html.match_indices(name).any(|(at, _)| {
                let start = html[..at].rfind('<').unwrap_or(0);
                let end = html[at..].find('>').map_or(html.len(), |end| at + end);
                html[start..end]
                    .split_once("value=\"")
                    .is_some_and(|(_, value)| !value.starts_with('"'))
            })
        })
}

/// One render's attempt at a Cloudflare challenge, run once the page loads
/// and again once it settles.
pub(crate) struct Solver {
    deadline: Instant,
    widget: Option<(Instant, bool)>,
}

impl Solver {
    pub(crate) fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            widget: None,
        }
    }

    /// Wait until `page` is no longer a Cloudflare challenge page, clicking
    /// the challenge's checkbox when one shows. A Turnstile widget embedded
    /// in the page gets clicked too, but the page is left as it is when no
    /// widget shows within 3 seconds, or none gives a token within 10 over
    /// the whole render, or the deadline passes. Fails when a challenge page
    /// is still shown at the deadline.
    pub(crate) async fn run(&mut self, page: &Page) -> Result<(), String> {
        solve(page, self.deadline, &mut self.widget).await
    }
}

async fn solve(
    page: &Page,
    deadline: Instant,
    widget: &mut Option<(Instant, bool)>,
) -> Result<(), String> {
    let mut shown: Option<Instant> = None;
    let mut clicked: Option<Instant> = None;
    loop {
        let html = page.content().await.unwrap_or_default();
        let kind = if html.is_empty() {
            Challenge::Wait
        } else {
            match challenge(&html) {
                Some(kind) => kind,
                None => return Ok(()),
            }
        };
        let now = Instant::now();
        if now >= deadline {
            if kind == Challenge::Widget {
                return Ok(());
            }
            return Err("the Cloudflare challenge was still shown when the time ran out".into());
        }
        let point = match kind {
            Challenge::Wait => None,
            _ => checkbox(page).await,
        };
        if kind == Challenge::Widget {
            let (since, seen) = widget.get_or_insert((now, false));
            *seen |= point.is_some();
            let waited = now.duration_since(*since);
            if waited >= WIDGET_PATIENCE || (!*seen && waited >= WIDGET_SHOWS_WITHIN) {
                return Ok(());
            }
        }
        if let Some(point) = point
            && clicked.is_none_or(|at| at.elapsed() >= RECLICK_AFTER)
            && shown.get_or_insert(now).elapsed() >= SHOWN_FOR
        {
            click(page, point).await;
            clicked = Some(Instant::now());
            shown = None;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn checkbox(page: &Page) -> Option<Point> {
    let root = page.get_document().await.ok()?;
    let frame = challenge_frame(&root)?;
    let model = page
        .execute(
            GetBoxModelParams::builder()
                .backend_node_id(frame.backend_node_id)
                .build(),
        )
        .await
        .ok()?
        .result
        .model;
    if model.width < 30 || model.height < 30 {
        return None;
    }
    let corner = model.border.inner();
    let middle = model.height as f64 / 2.0;
    let mut rng = rand::thread_rng();
    Some(Point {
        x: corner[0] + rng.gen_range(24.0..30.0),
        y: corner[1] + rng.gen_range(middle - 4.0..middle + 4.0),
    })
}

fn challenge_frame(node: &Node) -> Option<&Node> {
    let is_challenge = node.node_name.eq_ignore_ascii_case("iframe")
        && node.attributes.as_ref().is_some_and(|attributes| {
            attributes.chunks(2).any(|pair| {
                pair[0] == "src"
                    && pair
                        .get(1)
                        .is_some_and(|src| src.contains("/cdn-cgi/challenge-platform/"))
            })
        });
    if is_challenge {
        return Some(node);
    }
    node.children
        .iter()
        .flatten()
        .chain(node.shadow_roots.iter().flatten())
        .chain(node.content_document.as_deref())
        .find_map(challenge_frame)
}

async fn click(page: &Page, point: Point) {
    let hold = Duration::from_millis(rand::thread_rng().gen_range(80..180));
    let _ = page.move_mouse_smooth(point).await;
    let _ = page.click_and_hold(point, hold).await;
}

#[cfg(test)]
mod tests {
    use super::{Challenge, challenge};

    #[test]
    fn challenge_pages_and_widgets_are_told_apart() {
        let page = |kind: &str| {
            format!(
                "<html><head><title>Just a moment...</title></head><body><script>window._cf_chl_opt = {{cvId: '3', cType: '{kind}'}};</script></body></html>"
            )
        };
        assert_eq!(challenge(&page("non-interactive")), Some(Challenge::Wait));
        assert_eq!(challenge(&page("managed")), Some(Challenge::Checkbox));
        assert_eq!(challenge(&page("interactive")), Some(Challenge::Checkbox));
        let widget = r#"<script src="https://challenges.cloudflare.com/turnstile/v0/api.js"></script><div class="cf-turnstile"><input type="hidden" name="cf-turnstile-response" id="cf-chl-widget-a_response" value=""></div>"#;
        assert_eq!(challenge(widget), Some(Challenge::Widget));
        let solved = widget.replace(r#"value="""#, r#"value="0.abcdef""#);
        assert_eq!(challenge(&solved), None);
        let recaptcha = r#"<script src="https://challenges.cloudflare.com/turnstile/v0/api.js?compat=recaptcha"></script><input type="hidden" value="0.token" name="g-recaptcha-response">"#;
        assert_eq!(challenge(recaptcha), None);
        assert_eq!(challenge("<html><body><p>hello</p></body></html>"), None);
    }

    #[test]
    fn only_a_real_widget_counts_not_a_page_that_names_it() {
        let docs = r#"<html><body><p>Load <code>https://challenges.cloudflare.com/turnstile/v0/api.js</code> and add a <code>cf-turnstile</code> div.</p></body></html>"#;
        assert_eq!(challenge(docs), None);
        let script = r#"<html><head><script async src="https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit"></script></head><body></body></html>"#;
        assert_eq!(challenge(script), Some(Challenge::Widget));
        let element = r#"<html><body><form><div class="form-row cf-turnstile" data-sitekey="1x"></div></form></body></html>"#;
        assert_eq!(challenge(element), Some(Challenge::Widget));
        let lookalike = r#"<html><body><div class="cf-turnstile-wrapper"></div></body></html>"#;
        assert_eq!(challenge(lookalike), None);
    }
}
