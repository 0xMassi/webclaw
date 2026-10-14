//! Resource types a browser render can skip.

/// Kinds of subresources a render skips so the page downloads less. Scripts,
/// XHR and the document itself always load. Parse it from a comma list of
/// `images`, `fonts` and `media`, or `none`. Stylesheets always load: many
/// apps fetch CSS on demand and fail without it.
///
/// It fails the requests of the page and its same-site frames that Chrome
/// types as one of these kinds. Some still go out: everything inside a
/// cross-site iframe, an image loaded through `<object>`, `<link
/// rel=prefetch>` hints and the favicon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Block {
    /// Images, including `srcset` candidates and CSS backgrounds.
    pub images: bool,
    /// Web fonts.
    pub fonts: bool,
    /// Audio, video and their text tracks.
    pub media: bool,
}

impl std::str::FromStr for Block {
    type Err = String;

    fn from_str(list: &str) -> Result<Self, String> {
        let mut block = Self::default();
        for kind in list.split(',').map(str::trim).filter(|k| !k.is_empty()) {
            match kind.to_ascii_lowercase().as_str() {
                "images" => block.images = true,
                "fonts" => block.fonts = true,
                "media" => block.media = true,
                "none" => {}
                _ => {
                    return Err(format!(
                        "cannot block \"{kind}\": use images, fonts, media or none"
                    ));
                }
            }
        }
        Ok(block)
    }
}
