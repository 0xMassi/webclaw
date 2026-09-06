use webclaw_core::{ExtractionOptions, extract, extract_with_options};

#[test]
fn explicit_scope_survives_sparse_fallbacks_and_data_recovery() {
    for region in ["main", "article", "section role='main'"] {
        for words in [0, 5, 29, 30, 80, 199, 200, 220] {
            let tag = region.split_whitespace().next().unwrap();
            let inside = "selected ".repeat(words);
            let html = format!(
                r#"<html><body><div><h1>Outside title</h1><p>{}</p></div>
                <{region}>{inside}<p class="omit">Excluded phrase</p></{tag}>
                <script>window.__DATA__ = {{description: "An unrelated description from a script outside the requested region."}};</script>
                </body></html>"#,
                "outside ".repeat(600)
            );
            let result = extract_with_options(
                &html,
                None,
                &ExtractionOptions {
                    only_main_content: true,
                    exclude_selectors: vec![".omit".into()],
                    ..Default::default()
                },
            )
            .unwrap();
            for output in [&result.content.markdown, &result.content.plain_text] {
                assert!(
                    !output.contains("outside")
                        && !output.contains("Outside")
                        && !output.contains("unrelated")
                        && !output.contains("Excluded"),
                    "{region}/{words}: {output}"
                );
                assert_eq!(output.split_whitespace().count(), words);
            }
        }
    }
    let html =
        "<main>Main text</main><div id='chosen'>Selected text</div><footer>Footer text</footer>";
    let result = extract_with_options(
        html,
        None,
        &ExtractionOptions {
            only_main_content: true,
            include_selectors: vec!["#chosen".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.content.plain_text, "Selected text");
    assert!(
        extract_with_options(
            "<p>Text without a main element.</p>",
            None,
            &ExtractionOptions {
                only_main_content: true,
                ..Default::default()
            }
        )
        .unwrap()
        .content
        .markdown
        .contains("Text without")
    );
}

#[test]
fn fallback_content_obeys_exclusions_and_keeps_text_formats_in_sync() {
    let html = r#"<div class="modal"><h1>Cookie preferences</h1></div><main><p>A short useful article.</p></main><div class="omit"><h1>Hidden title</h1></div>"#;
    let result = extract_with_options(
        html,
        None,
        &ExtractionOptions {
            exclude_selectors: vec![".omit".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!result.content.markdown.contains("Hidden title"));
    let result = extract(html, None).unwrap();
    assert!(!result.content.markdown.contains("Cookie preferences"));
    assert!(result.content.plain_text.contains("Hidden title"));
}

#[test]
fn noscript_recovers_readable_html_but_not_tracking_or_excluded_content() {
    let html = r#"<html><body><main><noscript><h1>Community categories</h1><p>Browse questions and answers from our community.</p><p class="omit">Excluded fallback content</p><script>bad()</script></noscript></main><noscript><img src="/track.gif"></noscript></body></html>"#;
    let result = extract_with_options(
        html,
        Some("https://example.com"),
        &ExtractionOptions {
            only_main_content: true,
            exclude_selectors: vec![".omit".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        result.content.markdown.contains("Community categories"),
        "{}",
        result.content.markdown
    );
    assert!(
        !result.content.markdown.contains("Excluded") && !result.content.markdown.contains("bad()"),
        "{}",
        result.content.markdown
    );
    assert!(result.content.plain_text.contains("Browse questions"));
    let excluded = extract_with_options(
        html,
        None,
        &ExtractionOptions {
            exclude_selectors: vec!["noscript".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(excluded.content.markdown.is_empty());
    let duplicated = extract("<main><p>A useful page with enough readable text.</p><noscript><p>A useful page with enough readable text.</p></noscript></main>", None).unwrap();
    assert_eq!(
        duplicated.content.markdown.matches("A useful page").count(),
        1
    );
}

#[test]
fn application_dictionaries_are_not_recovered_as_page_content() {
    let html = r#"<html><body><main>\u{200b}</main><script>
    window.__InitialI18nStore__ = { title: "Long translated labels that do not describe this page." };
    window.__PRIVACY_CONFIG__ = { description: "Long privacy dialog text that is not page content." };
    window.__PageContext__ = { translations: { title: "Another irrelevant translation for an unrelated screen." }, productConfiguration: { description: "A warm wool sweater with ribbed cuffs and a round neck." } };
    </script></body></html>"#;
    let result = extract(html, None).unwrap();
    assert!(
        !result.content.markdown.contains("translated")
            && !result.content.markdown.contains("privacy")
            && !result.content.markdown.contains("translation")
    );
    #[cfg(feature = "quickjs")]
    assert!(result.content.markdown.contains("wool sweater"));
    assert_eq!(
        result.content.markdown.contains("wool sweater"),
        result.content.plain_text.contains("wool sweater")
    );
}

#[test]
fn unrelated_exclusions_preserve_recovered_hero_content() {
    let html = "<header><h1>Useful hero title</h1><p>A substantial and useful tagline explaining what the product does.</p></header><article><p>Short article text.</p></article><div class='cookie'>Hidden cookie text</div>";
    let result = extract_with_options(
        html,
        None,
        &ExtractionOptions {
            exclude_selectors: vec![".cookie".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(result.content.markdown.contains("Useful hero title"));
    assert!(
        result
            .content
            .markdown
            .contains("substantial and useful tagline")
    );
    assert!(!result.content.markdown.contains("Hidden cookie text"));
}
