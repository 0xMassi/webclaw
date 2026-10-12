//! robots.txt rules for one crawler, per RFC 9309.

use std::time::Duration;

/// Product token webclaw matches robots.txt groups against.
pub const USER_AGENT_TOKEN: &str = "webclaw";

/// The rules from one robots.txt that apply to webclaw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RobotsRules {
    rules: Vec<(bool, String)>,
    /// `Crawl-delay` from the applicable group, when present.
    pub crawl_delay: Option<Duration>,
}

impl RobotsRules {
    /// Rules that allow every path (robots.txt missing or 4xx).
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// Rules that disallow every path (robots.txt unreachable or 5xx).
    pub fn disallow_all() -> Self {
        Self {
            rules: vec![(false, "/".into())],
            crawl_delay: None,
        }
    }

    /// Parse a robots.txt body, keeping the group(s) for `agent`, or the `*`
    /// group(s) when no group names `agent`.
    pub fn parse(body: &str, agent: &str) -> Self {
        let agent = agent.to_ascii_lowercase();
        let mut specific = Self::default();
        let mut wildcard = Self::default();
        let mut found_specific = false;
        let mut group_agents: Vec<String> = Vec::new();
        let mut in_rules = false;

        for line in body.trim_start_matches('\u{feff}').lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            if key == "user-agent" {
                if in_rules {
                    group_agents.clear();
                    in_rules = false;
                }
                group_agents.push(value.to_ascii_lowercase());
                continue;
            }
            if group_agents.is_empty() {
                continue;
            }
            in_rules = true;
            let matches_us = group_agents.iter().any(|a| a == &agent);
            if matches_us {
                found_specific = true;
            }
            let target = if matches_us {
                &mut specific
            } else if group_agents.iter().any(|a| a == "*") {
                &mut wildcard
            } else {
                continue;
            };
            match key.as_str() {
                "allow" if !value.is_empty() => target.rules.push((true, value.into())),
                "disallow" if !value.is_empty() => target.rules.push((false, value.into())),
                "crawl-delay" => {
                    target.crawl_delay = value
                        .parse::<f64>()
                        .ok()
                        .filter(|s| s.is_finite() && *s >= 0.0)
                        .map(Duration::from_secs_f64);
                }
                _ => {}
            }
        }
        if found_specific { specific } else { wildcard }
    }

    /// Whether webclaw may fetch `path` (path plus query, starting with `/`).
    /// The longest matching rule wins; on a tie, allow wins.
    pub fn is_allowed(&self, path: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for (allow, pattern) in &self.rules {
            if pattern_matches(pattern, path) {
                let len = pattern.len();
                best = match best {
                    Some((l, a)) if l > len || (l == len && a) => Some((l, a)),
                    _ => Some((len, *allow)),
                };
            }
        }
        best.is_none_or(|(_, allow)| allow)
    }
}

// ponytail: no percent-encoding normalisation between pattern and path; add it if a site's rules depend on it.
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pattern, false),
    };
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    for (i, part) in parts.iter().enumerate() {
        if anchored && i == parts.len() - 1 {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    !anchored || rest.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "\u{feff}# comment
User-agent: *
Disallow: /private
Allow: /private/open
Disallow: /*.pdf$
Crawl-delay: 2.5

User-agent: otherbot
Disallow: /
";

    #[test]
    fn wildcard_group_applies_when_no_group_names_webclaw() {
        let rules = RobotsRules::parse(BODY, USER_AGENT_TOKEN);
        assert!(rules.is_allowed("/"));
        assert!(rules.is_allowed("/public/page"));
        assert!(!rules.is_allowed("/private"));
        assert!(!rules.is_allowed("/private/secret?x=1"));
        assert!(rules.is_allowed("/private/open/doc"));
        assert!(!rules.is_allowed("/files/report.pdf"));
        assert!(rules.is_allowed("/files/report.pdf?download=1"));
        assert_eq!(rules.crawl_delay, Some(Duration::from_millis(2500)));
    }

    #[test]
    fn a_group_naming_webclaw_replaces_the_wildcard_group() {
        let body = "User-agent: *\nDisallow: /\n\nuser-agent: WebClaw\nuser-agent: other\nDISALLOW: /admin\n";
        let rules = RobotsRules::parse(body, USER_AGENT_TOKEN);
        assert!(rules.is_allowed("/docs"));
        assert!(!rules.is_allowed("/admin/users"));
    }

    #[test]
    fn groups_for_the_same_agent_merge() {
        let body = "User-agent: *\nDisallow: /a\n\nUser-agent: *\nDisallow: /b\n";
        let rules = RobotsRules::parse(body, USER_AGENT_TOKEN);
        assert!(!rules.is_allowed("/a"));
        assert!(!rules.is_allowed("/b"));
        assert!(rules.is_allowed("/c"));
    }

    #[test]
    fn longest_match_wins_and_allow_wins_ties() {
        let body = "User-agent: *\nAllow: /page\nDisallow: /page\nDisallow: /p\nAllow: /\n";
        let rules = RobotsRules::parse(body, USER_AGENT_TOKEN);
        assert!(rules.is_allowed("/page/1"));
        assert!(!rules.is_allowed("/pricing"));
        assert!(rules.is_allowed("/about"));
    }

    #[test]
    fn empty_disallow_and_empty_file_allow_everything() {
        assert!(
            RobotsRules::parse("User-agent: *\nDisallow:\n", USER_AGENT_TOKEN).is_allowed("/x")
        );
        assert!(RobotsRules::parse("", USER_AGENT_TOKEN).is_allowed("/x"));
        assert!(RobotsRules::parse("Disallow: /\n", USER_AGENT_TOKEN).is_allowed("/x"));
    }

    #[test]
    fn wildcards_in_the_middle_match() {
        let rules = RobotsRules::parse("User-agent: *\nDisallow: /*/edit\n", USER_AGENT_TOKEN);
        assert!(!rules.is_allowed("/wiki/Rust/edit"));
        assert!(rules.is_allowed("/wiki/Rust"));
    }

    #[test]
    fn disallow_all_and_allow_all() {
        assert!(!RobotsRules::disallow_all().is_allowed("/"));
        assert!(RobotsRules::allow_all().is_allowed("/anything"));
    }

    #[test]
    fn ignores_a_malformed_crawl_delay() {
        let rules = RobotsRules::parse("User-agent: *\nCrawl-delay: soon\n", USER_AGENT_TOKEN);
        assert_eq!(rules.crawl_delay, None);
    }
}
