/// Shared noise detection for web content extraction.
///
/// Identifies elements that don't contribute to main content:
/// navigation, sidebars, footers, ads, cookie banners, modals, etc.
/// Used by both the extractor (candidate filtering) and the markdown
/// converter (output-time stripping).
use scraper::ElementRef;

const NOISE_TAGS: &[&str] = &[
    "script", "style", "noscript", "iframe", "svg", "nav", "aside", "footer", "header", "video",
    "audio", "canvas",
    // NOTE: <form> removed from this list — ASP.NET and similar frameworks wrap the
    // entire page body in a single <form> tag that contains all real content.
    // Forms are now handled with a heuristic in is_noise() that distinguishes
    // small input forms (noise) from page-wrapping forms (not noise).
    // NOTE: <picture> removed — it's a responsive image container, not noise.
    // <picture> wraps <source> and <img> for responsive images.
];

const NOISE_ROLES: &[&str] = &["navigation", "banner", "complementary", "contentinfo"];

const NOISE_CLASS_PATTERNS: &[&str] = &[
    "sidebar",
    "side",
    "nav",
    "navbar",
    "navigation",
    "menu",
    "footer",
    "header",
    "top",
    "bottom",
    "advertisement",
    "advert",
    "social",
    "social-media",
    "social-links",
    "share",
    "comment",
    "cookie",
    "popup",
    "modal",
    "overlay",
    "banner",
    "breadcrumb",
    "breadcrumbs",
    "widget",
    "lang-selector",
    "language",
    "newsletter",
    "subscribe",
    "related-posts",
    "recommended",
    "pagination",
    "pager",
    "signup",
    "login-form",
    "search-form",
    "notification",
    "alert",
    "toast",
    "skip-link",
    "sr-only",
    "visually-hidden",
];

const NOISE_ID_PATTERNS: &[&str] = &[
    "sidebar",
    "nav",
    "menu",
    "footer",
    "header",
    "cookie",
    "popup",
    "modal",
    "breadcrumbs",
    "widget",
    "language-selector",
    "ad",
    "social",
    "share",
    "newsletter",
    "subscribe",
    "comments",
    "related",
    "recommended",
];

/// Exact class tokens that indicate noise.
/// Unlike substring matching, these only match when the EXACT class token
/// is present — ".modal" matches `class="modal"` but NOT `class="free-modal-container"`.
const NOISE_CLASSES: &[&str] = &[
    "header",
    "top",
    "navbar",
    "footer",
    "bottom",
    "sidebar",
    "modal",
    "popup",
    "overlay",
    "ad",
    "ads",
    "advert",
    "lang-selector",
    "language",
    "social",
    "social-media",
    "social-links",
    "menu",
    "navigation",
    "breadcrumbs",
    "breadcrumb",
    "share",
    "widget",
    "cookie",
    "newsletter",
    "subscribe",
    "skip-link",
    "sr-only",
    "visually-hidden",
    "notification",
    "alert",
    "toast",
    "pagination",
    "pager",
    "signup",
    "login-form",
    "search-form",
    "related-posts",
    "recommended",
];

/// Exact IDs that indicate noise.
const NOISE_IDS: &[&str] = &[
    "header",
    "footer",
    "nav",
    "sidebar",
    "menu",
    "modal",
    "popup",
    "cookie",
    "breadcrumbs",
    "widget",
    "ad",
    "social",
    "share",
    "newsletter",
    "subscribe",
    "comments",
    "related",
    "recommended",
];

/// ID prefixes for cookie consent platforms that should be stripped entirely.
/// These generate massive DOM overlays that dominate content extraction.
const COOKIE_CONSENT_ID_PREFIXES: &[&str] = &[
    "onetrust",       // OneTrust (Foot Locker, many EU sites)
    "optanon",        // OneTrust legacy
    "ot-sdk",         // OneTrust SDK
    "cookiebot",      // Cookiebot
    "CybotCookiebot", // Cookiebot
    "cc-",            // Cookie Consent (Osano)
    "cookie-law",     // Cookie Law Info
    "gdpr",           // Generic GDPR banners
    "consent-",       // Generic consent banners
    "cmp-",           // Consent Management Platforms
    "sp_message",     // SourcePoint
    "qc-cmp",         // Quantcast CMP
    "trustarc",       // TrustArc
    "evidon",         // Evidon/Crownpeak
];

/// Check if an element is noise by tag, role, class, or id.
///
/// Uses EXACT class token matching instead
/// of substring matching. This prevents false positives like:
/// - "free-modal-container" ≠ noise (Vice.com's content wrapper)
/// - "a-bw_aui_cxc_alert_measurement" ≠ noise (Amazon's body class)
/// - "desktop" ≠ noise (not matching "top")
pub fn is_noise(el: ElementRef<'_>) -> bool {
    let tag = el.value().name();

    // Never treat <body> or <html> as noise.
    if tag == "body" || tag == "html" {
        return false;
    }

    // Tag-based noise (script, style, nav, etc.)
    if NOISE_TAGS.contains(&tag) {
        return true;
    }

    // <form> heuristic: ASP.NET wraps the entire page body in a single <form>.
    // These page-wrapping forms contain hundreds of words of real content.
    // Small forms (login, search, newsletter) are noise.
    if tag == "form" {
        let text_len = el.text().collect::<String>().len();
        // A form with substantial text (>500 chars) is likely a page wrapper, not noise.
        // Small forms (login/search/subscribe) rarely exceed a few hundred chars.
        if text_len < 500 {
            return true;
        }
        // Also check noise classes/IDs — a big form with class="login-form" is still noise
        if let Some(class) = el.value().attr("class") {
            let cl = class.to_lowercase();
            if cl.contains("login") || cl.contains("search") || cl.contains("subscribe")
                || cl.contains("signup") || cl.contains("newsletter") || cl.contains("contact")
            {
                return true;
            }
        }
        return false;
    }

    // ARIA role-based noise
    if let Some(role) = el.value().attr("role")
        && NOISE_ROLES.contains(&role)
    {
        return true;
    }

    // Exact class token matching — split class attribute into tokens,
    // check each against the noise list. "free-modal-container" splits into
    // ["free-modal-container"] which does NOT match "modal".
    if let Some(class) = el.value().attr("class") {
        let mut class_matched = false;
        for token in class.split_whitespace() {
            let lower = token.to_lowercase();
            if NOISE_CLASSES.contains(&lower.as_str()) {
                class_matched = true;
                break;
            }
            // Structural elements use compound names (FooterLinks, Header-nav, etc.)
            // These are always noise regardless of compound form.
            if lower.starts_with("footer")
                || lower.starts_with("header-")
                || lower.starts_with("nav-")
            {
                class_matched = true;
                break;
            }
        }
        if !class_matched {
            class_matched = is_ad_class(class);
        }

        if class_matched {
            // Safety valve: malformed HTML can leave noise containers unclosed,
            // causing them to absorb the entire page content. A real header/nav/
            // footer rarely exceeds a few thousand characters of text. If a
            // noise-class element has massive text content, it's almost certainly
            // a broken wrapper — treat it as content, not noise.
            let text_len = el.text().collect::<String>().len();
            if text_len > 5000 {
                return false;
            }
            return true;
        }
    }

    // Exact ID matching
    if let Some(id) = el.value().attr("id") {
        let id_lower = id.to_lowercase();
        if NOISE_IDS.contains(&id_lower.as_str()) && !is_structural_id(&id_lower) {
            // Same safety valve for ID-matched noise elements
            let text_len = el.text().collect::<String>().len();
            if text_len > 5000 {
                return false;
            }
            return true;
        }
        // Cookie consent platform IDs (prefix match — these generate huge overlays)
        for prefix in COOKIE_CONSENT_ID_PREFIXES {
            if id_lower.starts_with(prefix) {
                return true;
            }
        }
    }

    // Class-based cookie consent detection (prefix match for platform classes)
    if let Some(class) = el.value().attr("class") {
        let class_lower = class.to_lowercase();
        for prefix in COOKIE_CONSENT_ID_PREFIXES {
            if class_lower.contains(prefix) {
                return true;
            }
        }
    }

    false
}

/// Check if an element is inside a noise container.
pub fn is_noise_descendant(el: ElementRef<'_>) -> bool {
    let mut node = el.parent();
    while let Some(parent) = node {
        if let Some(parent_el) = ElementRef::wrap(parent)
            && is_noise(parent_el)
        {
            return true;
        }
        node = parent.parent();
    }
    false
}

fn has_noise_class(class: &str) -> bool {
    // Match noise patterns against individual class tokens, with safeguards
    // against Tailwind CSS utility classes that contain noise keywords as
    // substrings (e.g., "pt-header-h" is padding, not a header class).
    class.split_whitespace().any(is_noise_token) || is_ad_class(class)
}

/// Check if a single class token is a noise indicator.
/// Requires the noise pattern to be the *semantic core* of the token,
/// not embedded inside a Tailwind utility prefix or CSS variable.
fn is_noise_token(token: &str) -> bool {
    let t = token.to_lowercase();

    // Skip Tailwind arbitrary values and CSS variable references entirely
    if t.contains("[--") || t.contains("var(") {
        return false;
    }

    // Strip common Tailwind responsive/state prefixes (e.g., "lg:", "hover:", "md:")
    let core = t.rsplit_once(':').map_or(t.as_str(), |(_, c)| c);

    // The noise pattern should match the semantic name, not be buried inside
    // a utility like "pt-header-h" (padding) or "mt-nav-offset" (margin).
    // Tailwind utilities start with known prefixes; if the token starts with one,
    // it's a utility class, not a semantic class.
    const UTILITY_PREFIXES: &[&str] = &[
        "p-",
        "pt-",
        "pb-",
        "pl-",
        "pr-",
        "px-",
        "py-",
        "m-",
        "mt-",
        "mb-",
        "ml-",
        "mr-",
        "mx-",
        "my-",
        "w-",
        "h-",
        "min-",
        "max-",
        "top-",
        "left-",
        "right-",
        "bottom-",
        "z-",
        "gap-",
        "text-",
        "bg-",
        "border-",
        "rounded-",
        "flex-",
        "grid-",
        "col-",
        "row-",
        "opacity-",
        "transition-",
        "duration-",
        "delay-",
        "ease-",
        "translate-",
        "scale-",
        "rotate-",
        "origin-",
        "overflow-",
        "inset-",
        "space-",
        "divide-",
        "ring-",
        "shadow-",
        "outline-",
        "font-",
        "leading-",
        "tracking-",
        "decoration-",
    ];
    if UTILITY_PREFIXES.iter().any(|pfx| core.starts_with(pfx)) {
        return false;
    }

    // "banner" and "overlay" only match as prefix — they false-positive as
    // suffixes in BEM/Webflow component names (e.g., "package_banner" is a
    // product card, not an ad banner; "planet-overlay" is a visual effect).
    const PREFIX_ONLY: &[&str] = &["banner", "overlay"];

    // Short patterns (≤6 chars like "nav", "top", "header", "widget") require
    // word-boundary matching to avoid false positives on compound CSS class
    // names (e.g., "desktop" ≠ "top", "celwidget" ≠ "widget",
    // "_categoriesheader_active" ≠ semantic "header").
    // A word boundary is `-`, `_`, or start/end of string.
    // Longer patterns (7+ chars like "sidebar", "breadcrumb") are specific
    // enough that substring matching is safe.
    NOISE_CLASS_PATTERNS.iter().any(|p| {
        if PREFIX_ONLY.contains(p) {
            core == *p || core.starts_with(&format!("{p}-")) || core.starts_with(&format!("{p}_"))
        } else if p.len() <= 6 {
            is_word_boundary_match(core, p)
        } else {
            core.contains(p)
        }
    })
}

/// Check if `pattern` appears in `text` at a word boundary.
/// Word boundaries are `-`, `_`, or start/end of string.
/// e.g., "nav" matches "main-nav", "nav-bar", "nav" but NOT "canvas", "navbar".
fn is_word_boundary_match(text: &str, pattern: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = text[start..].find(pattern) {
        let abs = start + pos;
        let before_ok = abs == 0 || matches!(text.as_bytes()[abs - 1], b'-' | b'_');
        let end = abs + pattern.len();
        let after_ok = end == text.len() || matches!(text.as_bytes()[end], b'-' | b'_');
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// IDs like "modal-portal", "nav-root", "header-container" are structural
/// wrappers (React portals, app roots), not actual noise elements.
fn is_structural_id(id: &str) -> bool {
    const STRUCTURAL_SUFFIXES: &[&str] =
        &["portal", "root", "container", "wrapper", "mount", "app"];
    STRUCTURAL_SUFFIXES.iter().any(|s| id.contains(s))
}

// ---------------------------------------------------------------------------
// CSS class text detection (visible content that looks like class names)
// ---------------------------------------------------------------------------

/// CSS utility prefixes that indicate a word is a class name, not prose.
/// Covers Tailwind, Bootstrap-ish, and common utility-first patterns.
const CSS_CLASS_PREFIXES: &[&str] = &[
    "text-",
    "bg-",
    "px-",
    "py-",
    "pt-",
    "pb-",
    "pl-",
    "pr-",
    "p-",
    "mx-",
    "my-",
    "mt-",
    "mb-",
    "ml-",
    "mr-",
    "m-",
    "w-",
    "h-",
    "min-",
    "max-",
    "flex-",
    "grid-",
    "col-",
    "row-",
    "gap-",
    "space-",
    "rounded-",
    "shadow-",
    "border-",
    "ring-",
    "outline-",
    "font-",
    "tracking-",
    "leading-",
    "decoration-",
    "opacity-",
    "transition-",
    "duration-",
    "delay-",
    "ease-",
    "translate-",
    "scale-",
    "rotate-",
    "origin-",
    "overflow-",
    "inset-",
    "divide-",
    "z-",
    "top-",
    "left-",
    "right-",
    "bottom-",
    "sr-",
    "not-",
    "group-",
    "peer-",
    "placeholder-",
    "focus-",
    "hover-",
    "active-",
    "disabled-",
    "dark-",
    "sm-",
    "md-",
    "lg-",
    "xl-",
    "2xl-",
];

/// Exact single-word CSS utility class names (no prefix needed).
const CSS_CLASS_EXACT: &[&str] = &[
    "flex",
    "grid",
    "block",
    "inline",
    "hidden",
    "static",
    "fixed",
    "absolute",
    "relative",
    "sticky",
    "isolate",
    "container",
    "prose",
    "antialiased",
    "truncate",
    "uppercase",
    "lowercase",
    "capitalize",
    "italic",
    "underline",
    "overline",
    "invisible",
    "visible",
    "sr-only",
    "not-sr-only",
];

/// Tailwind responsive/state prefixes that can appear before a utility class
/// (e.g., "sm:text-lg", "hover:bg-blue-500", "dark:text-white").
fn strip_tw_variant_prefix(word: &str) -> &str {
    // Handle chained variants: "dark:sm:text-lg" → "text-lg"
    word.rsplit_once(':').map_or(word, |(_, core)| core)
}

/// Check if a single whitespace-delimited word looks like a CSS utility class.
fn is_css_class_word(word: &str) -> bool {
    let core = strip_tw_variant_prefix(word);
    let lower = core.to_lowercase();

    // Arbitrary value syntax: "[--foo:bar]", "w-[200px]"
    if lower.contains('[') && lower.contains(']') {
        return true;
    }

    // Exact matches
    if CSS_CLASS_EXACT.iter().any(|&e| lower == e) {
        return true;
    }

    // Prefix matches
    if CSS_CLASS_PREFIXES.iter().any(|pfx| lower.starts_with(pfx)) {
        return true;
    }

    // Negative utilities: "-mt-4", "-translate-x-1/2"
    if lower.starts_with('-') && lower.len() > 1 {
        let rest = &lower[1..];
        if CSS_CLASS_PREFIXES.iter().any(|pfx| rest.starts_with(pfx)) {
            return true;
        }
    }

    false
}

/// Public wrapper for single-word CSS class detection (used by LLM pipeline
/// for stripping trailing CSS classes from mixed-content lines).
pub fn is_css_class_word_pub(word: &str) -> bool {
    is_css_class_word(word)
}

/// Check if a text block is predominantly CSS class names.
///
/// Returns true if >50% of the whitespace-delimited words look like CSS
/// utility classes. Requires at least 3 words to avoid false positives on
/// short fragments.
pub fn is_css_class_text(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 3 {
        return false;
    }

    let css_count = words.iter().filter(|w| is_css_class_word(w)).count();
    // >50% of words are CSS classes
    css_count * 2 > words.len()
}

/// Detect "ad" as a standalone class token, not a substring of "read" or "loading".
fn is_ad_class(class: &str) -> bool {
    class.split_whitespace().any(|token| {
        token == "ad"
            || token.starts_with("ad-")
            || token.starts_with("ad_")
            || token.ends_with("-ad")
            || token.ends_with("_ad")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ad_class_standalone_detected() {
        assert!(is_ad_class("ad"));
        assert!(is_ad_class("some ad-banner"));
        assert!(is_ad_class("top-ad widget"));
        assert!(is_ad_class("ad_unit"));
        assert!(is_ad_class("sidebar_ad"));
    }

    #[test]
    fn ad_class_no_false_positive() {
        assert!(!is_ad_class("reading-time"));
        assert!(!is_ad_class("loading-indicator"));
        assert!(!is_ad_class("download-button"));
        assert!(!is_ad_class("breadcrumb"));
    }

    #[test]
    fn noise_class_patterns() {
        assert!(has_noise_class("main-sidebar"));
        assert!(has_noise_class("cookie-banner")); // "cookie" substring match
        assert!(has_noise_class("modal-overlay")); // "modal" substring match
        assert!(has_noise_class("banner-top")); // "banner" as prefix
        assert!(has_noise_class("overlay-popup")); // "overlay" as prefix
        assert!(!has_noise_class("article-content"));
        assert!(!has_noise_class("post-body"));
    }

    #[test]
    fn short_patterns_require_word_boundary() {
        // "nav" (3 chars) — must be a standalone word segment
        assert!(has_noise_class("main-nav"));
        assert!(has_noise_class("nav-bar"));
        assert!(has_noise_class("nav"));
        assert!(!has_noise_class("canvas")); // "nav" is substring, not word
        assert!(has_noise_class("icp-nav-flag")); // "nav" IS between word boundaries
        // "top" (3 chars) — note: "top-bar" starts with Tailwind prefix "top-" → filtered out
        assert!(has_noise_class("page-top")); // "top" at word boundary
        assert!(!has_noise_class("desktop")); // "top" is substring inside word
        assert!(!has_noise_class("stop-motion")); // "top" inside word
        // "side" (4 chars) — "left-side" starts with Tailwind prefix "left-" → filtered
        assert!(has_noise_class("page-side"));
        assert!(!has_noise_class("inside-content"));
        assert!(!has_noise_class("consider"));
    }

    #[test]
    fn amazon_classes_not_noise() {
        // Amazon CSS module class names that were false-positiving
        assert!(!has_noise_class("desktop")); // contains "top"
        assert!(!has_noise_class("celwidget")); // contains "widget"
        // a-alert-container: "alert" IS a proper word segment → still matches (correct for UI alerts)
        assert!(has_noise_class("a-alert-container"));
        assert!(!has_noise_class(
            "_haul-cx-images-carousel_style_desktop-card__fid8k"
        ));
        assert!(!has_noise_class(
            "_haul-cx-infinite-scroll-body_categoriesheader_active__2j-4u"
        ));
        // But actual noise classes still work
        assert!(has_noise_class("site-header"));
        assert!(has_noise_class("main-nav"));
        assert!(has_noise_class("footer-links"));
        assert!(has_noise_class("cookie-consent"));
    }

    #[test]
    fn word_boundary_match_works() {
        assert!(is_word_boundary_match("main-nav", "nav"));
        assert!(is_word_boundary_match("nav-bar", "nav"));
        assert!(is_word_boundary_match("nav", "nav"));
        assert!(is_word_boundary_match("top-nav_bar", "nav"));
        assert!(!is_word_boundary_match("canvas", "nav"));
        assert!(!is_word_boundary_match("navbar", "nav"));
        assert!(!is_word_boundary_match("navigate", "nav"));
        assert!(is_word_boundary_match("top-bar", "top"));
        assert!(!is_word_boundary_match("desktop", "top"));
        assert!(!is_word_boundary_match("stopper", "top"));
    }

    #[test]
    fn bem_component_names_not_noise() {
        // BEM/Webflow component names where noise keyword is a suffix
        assert!(!has_noise_class("package_banner"));
        assert!(!has_noise_class("mars-cta_planet-overlay"));
        assert!(!has_noise_class("hero_banner_wrap"));
        // But actual noise classes still work
        assert!(has_noise_class("banner-dismiss"));
        assert!(has_noise_class("overlay-backdrop"));
    }

    #[test]
    fn structural_ids_not_noise() {
        assert!(is_structural_id("modal-portal"));
        assert!(is_structural_id("nav-root"));
        assert!(is_structural_id("header-container"));
        assert!(is_structural_id("sidebar-wrapper"));
        assert!(is_structural_id("menu-mount"));
        assert!(is_structural_id("app"));
        // Actual noise IDs should NOT be structural
        assert!(!is_structural_id("main-sidebar"));
        assert!(!is_structural_id("cookie-consent"));
        assert!(!is_structural_id("popup-overlay"));
    }

    #[test]
    fn tailwind_animation_utilities_not_noise() {
        // Tailwind transition/animation utilities with noise keywords as values
        assert!(!has_noise_class("ease-curve-sidebar"));
        assert!(!has_noise_class("duration-sidebar"));
        assert!(!has_noise_class("delay-modal-open"));
        // But actual sidebar/modal classes still work
        assert!(has_noise_class("sidebar-panel"));
        assert!(has_noise_class("modal-dialog"));
    }

    #[test]
    fn tailwind_css_vars_not_noise() {
        // Tailwind arbitrary values and CSS variables should NOT trigger noise
        assert!(!has_noise_class("[--content-top-offset:var(--header-h)]"));
        assert!(!has_noise_class(
            "pt-[var(--content-top-offset)] [--content-top-offset:var(--header-h)]"
        ));
        assert!(!has_noise_class("[--nav-width:200px]"));
        // But actual noise classes still work
        assert!(has_noise_class("[--offset:10px] header-bar"));
        assert!(has_noise_class("sidebar [--x:1]"));
    }

    // -----------------------------------------------------------------------
    // CSS class text detection (decorative text that looks like class names)
    // -----------------------------------------------------------------------

    #[test]
    fn css_class_text_detected() {
        // Pure Tailwind utility class blocks — the real-world problem
        assert!(is_css_class_text(
            "text-4xl font-bold tracking-tight text-gray-900"
        ));
        assert!(is_css_class_text(
            "text-4xl text-5xl text-6xl text-8xl text-gray-950 text-white tracking-tighter text-balance"
        ));
        assert!(is_css_class_text(
            "flex grid rounded-lg shadow-md bg-white px-4 py-2"
        ));
        assert!(is_css_class_text(
            "sm:text-lg dark:bg-gray-800 hover:bg-blue-500"
        ));
        // Negative utilities
        assert!(is_css_class_text("-mt-4 -translate-x-1/2 flex"));
    }

    #[test]
    fn css_class_text_normal_prose_kept() {
        // Normal English text — must NOT be detected as CSS
        assert!(!is_css_class_text(
            "the text-based approach works well for this use case"
        ));
        assert!(!is_css_class_text(
            "Build beautiful websites with modern tools"
        ));
        assert!(!is_css_class_text(
            "Tailwind CSS is a utility-first CSS framework"
        ));
        // Too short to be confident
        assert!(!is_css_class_text("flex grid"));
        assert!(!is_css_class_text("text-lg"));
    }

    #[test]
    fn css_class_text_mixed_content() {
        // Majority CSS → detected
        assert!(is_css_class_text(
            "text-4xl font-bold tracking-tight text-gray-900 hero"
        ));
        // Majority prose → not detected
        assert!(!is_css_class_text(
            "The quick brown fox jumps over the lazy text-lg dog"
        ));
    }
}

#[cfg(test)]
mod form_tests {
    use super::*;
    use scraper::Html;

    #[test]
    fn aspnet_page_wrapping_form_is_not_noise() {
        let html = r#"<html><body><form method="post" action="./page.aspx" id="form1"><div class="wrapper"><div class="content"><h1>Support</h1><h3>Question one?</h3><p>Long answer text that should definitely be captured by the extraction engine. This is real content with multiple sentences to ensure it passes any text length thresholds in the scoring algorithm. We need at least five hundred characters of actual text content here to exceed the threshold. Adding more sentences about various topics including data formats, historical prices, stock market analysis, technical indicators, and trading strategies. This paragraph discusses how intraday data can be used for backtesting quantitative models and developing automated trading systems.</p><h3>Question two?</h3><p>Another substantial answer paragraph with detailed information about the product features and capabilities.</p></div></div></form></body></html>"#;
        let doc = Html::parse_document(html);
        let form = doc.select(&scraper::Selector::parse("form").unwrap()).next().unwrap();
        let text = form.text().collect::<String>();
        let text_len = text.len();
        assert!(text_len >= 500, "Form text should be >= 500 chars, got {text_len}");
        assert!(!is_noise(form), "ASP.NET page-wrapping form should NOT be noise");
    }

    #[test]
    fn small_login_form_is_noise() {
        let html = r#"
        <html><body>
        <form action="/login">
            <input type="text" name="user" />
            <input type="password" name="pass" />
            <button>Login</button>
        </form>
        </body></html>
        "#;
        let doc = Html::parse_document(html);
        let form = doc.select(&scraper::Selector::parse("form").unwrap()).next().unwrap();
        assert!(is_noise(form), "Small login form SHOULD be noise");
    }
}
