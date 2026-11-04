use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Value, json};

use crate::tools::ScrapeParams;

pub(crate) const NO_BROWSER: &str = "rendering needs a browser and this webclaw-mcp build has none: set WEBCLAW_API_KEY to render through the hosted API, or build webclaw-mcp with `--features browser`";

pub(crate) const UNRENDERED_NOTICE: &str = "Notice: this page needs JavaScript to show its content, so what came back may be incomplete. Set WEBCLAW_API_KEY to render it through the hosted API, or build webclaw-mcp with `--features browser` to render it locally.";

pub(crate) const PNG_ONLY: &str = "the hosted API returns PNG screenshots only: use format \"png\", or build webclaw-mcp with `--features browser` for JPEG, PDF and MHTML";

const WAIT_FOR_MS: u128 = crate::server::LOCAL_FETCH_TIMEOUT.as_millis();

const STEP_KEYS: &[(&str, &[&str])] = &[
    ("click", &["selector"]),
    ("type", &["selector", "value"]),
    ("press", &["key"]),
    ("scroll", &["direction", "amount"]),
    ("wait", &["milliseconds"]),
    ("waitForSelector", &["selector", "timeout"]),
    ("executeJavascript", &["code"]),
];

fn steps(wait_for: Option<&str>, actions: Option<&[Value]>) -> Result<Vec<Value>, String> {
    let mut steps: Vec<Value> = wait_for
        .map(|selector| json!({"type": "waitForSelector", "selector": selector, "timeout": WAIT_FOR_MS}))
        .into_iter()
        .collect();
    for (n, step) in actions.unwrap_or_default().iter().enumerate() {
        let kind = step.get("type").and_then(Value::as_str).unwrap_or_default();
        let keys = STEP_KEYS
            .iter()
            .find(|(name, _)| *name == kind)
            .map(|(_, keys)| *keys)
            .ok_or_else(|| format!("step {}: unknown step type \"{kind}\"", n + 1))?;
        if let Some(key) = step
            .as_object()
            .into_iter()
            .flat_map(|o| o.keys())
            .find(|key| *key != "type" && !keys.contains(&key.as_str()))
        {
            return Err(format!(
                "step {}: the hosted API takes no \"{key}\" on a {kind} step; drop it, or build webclaw-mcp with `--features browser` to run it locally",
                n + 1
            ));
        }
        steps.push(step.clone());
    }
    Ok(steps)
}

pub(crate) fn scrape_body(
    params: &ScrapeParams,
    format: &str,
    keyed: bool,
) -> Result<Option<Value>, String> {
    let render = params.render.as_deref();
    if !matches!(render, None | Some("auto" | "always" | "never")) {
        return Err("render must be \"auto\", \"always\" or \"never\"".into());
    }
    if let Some(block) = params.block.as_deref() {
        block.parse::<webclaw_fetch::Block>()?;
    }
    let actions = params.actions.as_deref().filter(|a| !a.is_empty());
    let solve = params.solve == Some(true);
    if render == Some("never") {
        if solve || actions.is_some() || params.capture_network.is_some() {
            return Err("solve, actions and capture_network need rendering, so they cannot run with render \"never\"".into());
        }
        return Ok(None);
    }
    if render != Some("always")
        && params.wait_for.is_none()
        && !solve
        && actions.is_none()
        && params.capture_network.is_none()
    {
        return Ok(None);
    }
    if params.capture_network.is_some() {
        return Err("capture_network needs a local browser, which this webclaw-mcp build does not have: build it with `--features browser`, or read the data with an executeJavascript step in `actions`".into());
    }
    if !keyed {
        return Err(NO_BROWSER.into());
    }
    if params.cookies.as_ref().is_some_and(|c| !c.is_empty()) {
        return Err("cookies are not sent to the hosted API, so rendering there would read the page without them: drop the cookies, or build webclaw-mcp with `--features browser` to render with them locally".into());
    }
    if params.chunk.is_some()
        && actions
            .unwrap_or_default()
            .iter()
            .any(|a| a["type"] == "executeJavascript")
    {
        return Err("chunk returns only the page's chunks, so it cannot carry js_results: drop the executeJavascript steps or chunk".into());
    }
    let mut steps = steps(params.wait_for.as_deref(), actions)?;
    if steps.is_empty() && render == Some("always") {
        steps.push(json!({"type": "wait", "milliseconds": 0}));
    }
    let mut body = json!({
        "url": params.url,
        "formats": [format],
        "include_selectors": params.include_selectors.clone().unwrap_or_default(),
        "exclude_selectors": params.exclude_selectors.clone().unwrap_or_default(),
        "only_main_content": params.only_main_content.unwrap_or(false),
    });
    if !steps.is_empty() {
        body["actions"] = json!(steps);
    }
    Ok(Some(body))
}

pub(crate) fn screenshot_body(
    url: &str,
    wait_for: Option<&str>,
    full_page: bool,
    actions: Option<&[Value]>,
) -> Result<Value, String> {
    let mut steps = steps(wait_for, actions)?;
    if full_page {
        steps.push(json!({"type": "screenshot", "fullPage": true}));
    }
    Ok(json!({"url": url, "formats": ["markdown"], "screenshot": true, "actions": steps}))
}

pub(crate) fn screenshot_result(resp: &Value) -> Result<CallToolResult, String> {
    let shot = resp
        .get("screenshot")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("the hosted API returned no screenshot")?;
    let (mime, data) = shot
        .strip_prefix("data:")
        .and_then(|s| s.split_once(";base64,"))
        .unwrap_or(("image/png", shot));
    let url = resp.get("url").and_then(Value::as_str).unwrap_or_default();
    let mut content = vec![ContentBlock::image(data, mime), ContentBlock::text(url)];
    if let Some(results) = crate::server::js_results(resp) {
        content.push(ContentBlock::text(results));
    }
    Ok(CallToolResult::success(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(extra: Value) -> ScrapeParams {
        let mut args = json!({"url": "https://example.com/js/"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(args).unwrap()
    }

    #[test]
    fn plain_scrapes_stay_local_with_or_without_a_key() {
        for extra in [
            json!({}),
            json!({"render": "auto"}),
            json!({"render": "never", "wait_for": ".x"}),
            json!({"block": "images"}),
            json!({"solve": false, "actions": []}),
        ] {
            for keyed in [false, true] {
                assert_eq!(
                    scrape_body(&params(extra.clone()), "markdown", keyed),
                    Ok(None),
                    "{extra}"
                );
            }
        }
    }

    #[test]
    fn rendering_without_a_key_says_how_to_get_it() {
        for extra in [
            json!({"render": "always"}),
            json!({"wait_for": ".quote"}),
            json!({"solve": true}),
            json!({"actions": [{"type": "click", "selector": "a"}]}),
        ] {
            let err = scrape_body(&params(extra.clone()), "markdown", false).unwrap_err();
            assert!(
                err.contains("WEBCLAW_API_KEY") && err.contains("--features browser"),
                "{extra} -> {err}"
            );
        }
    }

    #[test]
    fn render_always_asks_the_hosted_api_for_a_browser() {
        let body = scrape_body(&params(json!({"render": "always"})), "llm", true)
            .unwrap()
            .unwrap();
        assert_eq!(body["url"], "https://example.com/js/");
        assert_eq!(body["formats"], json!(["llm"]));
        assert_eq!(
            body["actions"],
            json!([{"type": "wait", "milliseconds": 0}])
        );
    }

    #[test]
    fn wait_for_runs_before_the_callers_actions() {
        let body = scrape_body(
            &params(json!({
                "wait_for": ".quote",
                "only_main_content": true,
                "actions": [{"type": "scroll", "direction": "down"}, {"type": "executeJavascript", "code": "1"}],
            })),
            "markdown",
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            body["actions"],
            json!([
                {"type": "waitForSelector", "selector": ".quote", "timeout": 30000},
                {"type": "scroll", "direction": "down"},
                {"type": "executeJavascript", "code": "1"},
            ])
        );
        assert_eq!(body["only_main_content"], true);
    }

    #[test]
    fn solve_leaves_the_challenge_to_the_hosted_api() {
        let body = scrape_body(&params(json!({"solve": true})), "markdown", true)
            .unwrap()
            .unwrap();
        assert!(body.get("actions").is_none(), "{body}");
    }

    #[test]
    fn what_the_hosted_api_cannot_do_is_refused_with_the_way_out() {
        for (extra, words) in [
            (
                json!({"capture_network": ""}),
                ["capture_network", "--features browser"],
            ),
            (
                json!({"render": "always", "cookies": ["a=b"]}),
                ["cookies", "--features browser"],
            ),
            (
                json!({"actions": [{"type": "click", "selector": "a", "repeat": 3}]}),
                ["\"repeat\"", "--features browser"],
            ),
            (
                json!({"actions": [{"type": "hover", "selector": "a"}]}),
                ["hover", "step 1"],
            ),
            (
                json!({"chunk": 100, "actions": [{"type": "executeJavascript", "code": "1"}]}),
                ["js_results", "chunk"],
            ),
            (
                json!({"render": "never", "solve": true}),
                ["render \"never\"", "solve"],
            ),
            (json!({"render": "sometimes"}), ["render must be", "auto"]),
        ] {
            let err = scrape_body(&params(extra.clone()), "markdown", true).unwrap_err();
            assert!(words.iter().all(|w| err.contains(w)), "{extra} -> {err}");
        }
        let err =
            scrape_body(&params(json!({"capture_network": ""})), "markdown", false).unwrap_err();
        assert!(err.contains("capture_network"), "{err}");
    }

    #[test]
    fn screenshots_ask_for_a_full_page_capture_last() {
        let body = screenshot_body(
            "https://example.com/",
            Some("main"),
            true,
            Some(&[json!({"type": "click", "selector": "#more"})]),
        )
        .unwrap();
        assert_eq!(body["screenshot"], true);
        assert_eq!(
            body["actions"],
            json!([
                {"type": "waitForSelector", "selector": "main", "timeout": 30000},
                {"type": "click", "selector": "#more"},
                {"type": "screenshot", "fullPage": true},
            ])
        );
        let viewport = screenshot_body("https://example.com/", None, false, None).unwrap();
        assert_eq!(viewport["actions"], json!([]));
    }

    #[test]
    fn a_hosted_screenshot_comes_back_as_an_image() {
        let result = screenshot_result(&json!({
            "url": "https://example.com/final",
            "screenshot": "data:image/png;base64,iVBORw0KGgo=",
            "js_results": ["Example"],
        }))
        .unwrap();
        let out = serde_json::to_value(&result).unwrap();
        assert_eq!(out["content"][0]["type"], "image");
        assert_eq!(out["content"][0]["data"], "iVBORw0KGgo=");
        assert_eq!(out["content"][0]["mimeType"], "image/png");
        assert_eq!(out["content"][1]["text"], "https://example.com/final");
        assert!(
            out["content"][2]["text"]
                .as_str()
                .unwrap()
                .contains("Example")
        );
        assert!(screenshot_result(&json!({"url": "https://example.com/"})).is_err());
    }
}
