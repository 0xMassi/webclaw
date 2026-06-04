/// Core types for extraction output.
/// All types are serializable for JSON output to LLM consumers.
use serde::{Deserialize, Serialize};

use crate::domain::DomainType;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExtractionResult {
    pub metadata: Metadata,
    pub content: Content,
    pub domain_data: Option<DomainData>,
    /// JSON-LD structured data extracted from `<script type="application/ld+json">` blocks.
    /// Contains Schema.org markup (Product, Article, BreadcrumbList, etc.) when present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub structured_data: Vec<serde_json::Value>,
}

impl ExtractionResult {
    /// Construct a result from metadata and content, defaulting
    /// `domain_data` to `None` and `structured_data` to empty.
    ///
    /// `ExtractionResult` is `#[non_exhaustive]`, so downstream crates must
    /// build it through this constructor instead of a struct literal.
    pub fn new(metadata: Metadata, content: Content) -> Self {
        Self {
            metadata,
            content,
            domain_data: None,
            structured_data: Vec::new(),
        }
    }

    /// Attach domain-specific data.
    #[must_use]
    pub fn with_domain_data(mut self, domain_data: Option<DomainData>) -> Self {
        self.domain_data = domain_data;
        self
    }

    /// Attach JSON-LD structured data blocks.
    #[must_use]
    pub fn with_structured_data(mut self, structured_data: Vec<serde_json::Value>) -> Self {
        self.structured_data = structured_data;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Metadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub published_date: Option<String>,
    pub language: Option<String>,
    pub url: Option<String>,
    pub site_name: Option<String>,
    pub image: Option<String>,
    pub favicon: Option<String>,
    pub word_count: usize,
}

impl Metadata {
    /// Start from an all-default `Metadata`. `Metadata` is `#[non_exhaustive]`,
    /// so downstream crates build it via `Metadata::default()` plus the
    /// `with_*` setters rather than a struct literal.
    #[must_use]
    pub fn with_title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    #[must_use]
    pub fn with_description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    #[must_use]
    pub fn with_author(mut self, author: Option<String>) -> Self {
        self.author = author;
        self
    }

    #[must_use]
    pub fn with_published_date(mut self, published_date: Option<String>) -> Self {
        self.published_date = published_date;
        self
    }

    #[must_use]
    pub fn with_language(mut self, language: Option<String>) -> Self {
        self.language = language;
        self
    }

    #[must_use]
    pub fn with_url(mut self, url: Option<String>) -> Self {
        self.url = url;
        self
    }

    #[must_use]
    pub fn with_site_name(mut self, site_name: Option<String>) -> Self {
        self.site_name = site_name;
        self
    }

    #[must_use]
    pub fn with_image(mut self, image: Option<String>) -> Self {
        self.image = image;
        self
    }

    #[must_use]
    pub fn with_favicon(mut self, favicon: Option<String>) -> Self {
        self.favicon = favicon;
        self
    }

    #[must_use]
    pub fn with_word_count(mut self, word_count: usize) -> Self {
        self.word_count = word_count;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Content {
    pub markdown: String,
    pub plain_text: String,
    pub links: Vec<Link>,
    pub images: Vec<Image>,
    pub code_blocks: Vec<CodeBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_html: Option<String>,
}

impl Content {
    /// Start from an all-default `Content`. `Content` is `#[non_exhaustive]`,
    /// so downstream crates build it via `Content::default()` plus the
    /// `with_*` setters rather than a struct literal.
    #[must_use]
    pub fn with_markdown(mut self, markdown: String) -> Self {
        self.markdown = markdown;
        self
    }

    #[must_use]
    pub fn with_plain_text(mut self, plain_text: String) -> Self {
        self.plain_text = plain_text;
        self
    }

    #[must_use]
    pub fn with_links(mut self, links: Vec<Link>) -> Self {
        self.links = links;
        self
    }

    #[must_use]
    pub fn with_images(mut self, images: Vec<Image>) -> Self {
        self.images = images;
        self
    }

    #[must_use]
    pub fn with_code_blocks(mut self, code_blocks: Vec<CodeBlock>) -> Self {
        self.code_blocks = code_blocks;
        self
    }

    #[must_use]
    pub fn with_raw_html(mut self, raw_html: Option<String>) -> Self {
        self.raw_html = raw_html;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Link {
    pub text: String,
    pub href: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub alt: String,
    pub src: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeBlock {
    pub language: Option<String>,
    pub code: String,
}

/// Domain-specific extracted data. For MVP, only the detected type is stored.
/// Future: each variant carries structured fields (e.g., Article { author, date, ... }).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainData {
    pub domain_type: DomainType,
}

/// Options for controlling content extraction behavior.
#[derive(Debug, Clone, Default)]
pub struct ExtractionOptions {
    /// CSS selectors for elements to include. If non-empty, only these elements
    /// are extracted (skipping the scoring algorithm entirely).
    pub include_selectors: Vec<String>,
    /// CSS selectors for elements to exclude from the output.
    pub exclude_selectors: Vec<String>,
    /// If true, skip scoring and pick the first `article`, `main`, or `[role="main"]` element.
    pub only_main_content: bool,
    /// If true, populate `Content::raw_html` with the extracted content's HTML.
    pub include_raw_html: bool,
}
