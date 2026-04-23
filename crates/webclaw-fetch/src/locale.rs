//! Derive an `Accept-Language` header from a URL.
//!
//! DataDome-class bot detection on country-specific sites (e.g. immobiliare.it,
//! leboncoin.fr) does a geo-vs-locale sanity check: residential IP in the
//! target country + a browser UA but the wrong `Accept-Language` is a bot
//! signal. Matching the site's expected locale gets us through.
//!
//! Default for unmapped TLDs is `en-US,en;q=0.9` — the global fallback.

/// Best-effort `Accept-Language` header value for the given URL's TLD.
/// Returns `None` if the URL cannot be parsed.
pub fn accept_language_for_url(url: &str) -> Option<&'static str> {
    let host = url::Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    let tld = host.rsplit('.').next()?;
    Some(accept_language_for_tld(tld))
}

/// Map a bare TLD like `it`, `fr`, `de` to a plausible `Accept-Language`.
/// Unknown TLDs fall back to US English.
pub fn accept_language_for_tld(tld: &str) -> &'static str {
    match tld {
        "it" => "it-IT,it;q=0.9",
        "fr" => "fr-FR,fr;q=0.9",
        "de" | "at" => "de-DE,de;q=0.9",
        "es" => "es-ES,es;q=0.9",
        "pt" => "pt-PT,pt;q=0.9",
        "nl" => "nl-NL,nl;q=0.9",
        "pl" => "pl-PL,pl;q=0.9",
        "se" => "sv-SE,sv;q=0.9",
        "no" => "nb-NO,nb;q=0.9",
        "dk" => "da-DK,da;q=0.9",
        "fi" => "fi-FI,fi;q=0.9",
        "cz" => "cs-CZ,cs;q=0.9",
        "ro" => "ro-RO,ro;q=0.9",
        "gr" => "el-GR,el;q=0.9",
        "tr" => "tr-TR,tr;q=0.9",
        "ru" => "ru-RU,ru;q=0.9",
        "jp" => "ja-JP,ja;q=0.9",
        "kr" => "ko-KR,ko;q=0.9",
        "cn" => "zh-CN,zh;q=0.9",
        "tw" | "hk" => "zh-TW,zh;q=0.9",
        "br" => "pt-BR,pt;q=0.9",
        "mx" | "ar" | "co" | "cl" | "pe" => "es-ES,es;q=0.9",
        "uk" | "ie" => "en-GB,en;q=0.9",
        _ => "en-US,en;q=0.9",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tld_dispatch() {
        assert_eq!(
            accept_language_for_url("https://www.immobiliare.it/annunci/1"),
            Some("it-IT,it;q=0.9")
        );
        assert_eq!(
            accept_language_for_url("https://www.leboncoin.fr/"),
            Some("fr-FR,fr;q=0.9")
        );
        assert_eq!(
            accept_language_for_url("https://www.amazon.co.uk/"),
            Some("en-GB,en;q=0.9")
        );
        assert_eq!(
            accept_language_for_url("https://example.com/"),
            Some("en-US,en;q=0.9")
        );
    }

    #[test]
    fn bad_url_returns_none() {
        assert_eq!(accept_language_for_url("not-a-url"), None);
    }
}
