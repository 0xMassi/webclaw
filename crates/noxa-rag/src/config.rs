use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::error::RagError;

/// Top-level configuration deserialized from noxa-rag.toml.
#[derive(Debug, Clone, Deserialize)]
pub struct RagConfig {
    pub source: SourceConfig,
    pub embed_provider: EmbedProviderConfig,
    pub vector_store: VectorStoreConfig,
    pub chunker: ChunkerConfig,
    pub pipeline: PipelineConfig,
    /// UUID namespace for deterministic point IDs.
    /// Default: 6ba7b810-9dad-11d1-80b4-00c04fd430c8
    #[serde(default = "default_uuid_namespace")]
    pub uuid_namespace: uuid::Uuid,
}

fn default_uuid_namespace() -> uuid::Uuid {
    uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceConfig {
    FsWatcher {
        watch_dir: PathBuf,
        #[serde(default = "default_debounce_ms")]
        debounce_ms: u64,
    },
}

fn default_debounce_ms() -> u64 {
    500
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmbedProviderConfig {
    Tei {
        url: String,
        model: String,
        /// Optional: load tokenizer from local path (avoids HF Hub at startup).
        local_path: Option<PathBuf>,
    },
    OpenAi {
        api_key: String,
        model: String,
    },
    VoyageAi {
        api_key: String,
        model: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VectorStoreConfig {
    Qdrant {
        /// gRPC URL — port 6334. qdrant-client v1.x uses gRPC (tonic), NOT REST.
        url: String,
        collection: String,
        /// Optional API key. Override with NOXA_RAG_QDRANT_API_KEY env var.
        api_key: Option<String>,
    },
    /// Dev/test only — factory returns RagError::Config("not implemented").
    InMemory,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChunkerConfig {
    #[serde(default = "default_target_tokens")]
    pub target_tokens: usize,
    #[serde(default = "default_overlap_tokens")]
    pub overlap_tokens: usize,
    #[serde(default = "default_min_words")]
    pub min_words: usize,
    #[serde(default = "default_max_chunks_per_page")]
    pub max_chunks_per_page: usize,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            target_tokens: default_target_tokens(),
            overlap_tokens: default_overlap_tokens(),
            min_words: default_min_words(),
            max_chunks_per_page: default_max_chunks_per_page(),
        }
    }
}

fn default_target_tokens() -> usize { 512 }
fn default_overlap_tokens() -> usize { 64 }
fn default_min_words() -> usize { 50 }
fn default_max_chunks_per_page() -> usize { 100 }

#[derive(Debug, Clone, Deserialize)]
pub struct PipelineConfig {
    #[serde(default = "default_embed_concurrency")]
    pub embed_concurrency: usize,
    /// MUST be an absolute path — systemd daemon runs with CWD = /.
    pub failed_jobs_log: Option<PathBuf>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            embed_concurrency: default_embed_concurrency(),
            failed_jobs_log: None,
        }
    }
}

fn default_embed_concurrency() -> usize { 4 }

/// Load and validate config from a TOML file.
pub fn load_config(path: &Path) -> Result<RagConfig, RagError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| RagError::Config(format!("cannot read config file {}: {}", path.display(), e)))?;

    let config: RagConfig = toml::from_str(&content)
        .map_err(|e| RagError::Config(format!("config parse error: {}", e)))?;

    // Validate embed_concurrency > 0
    if config.pipeline.embed_concurrency == 0 {
        return Err(RagError::Config(
            "pipeline.embed_concurrency must be > 0 (0 causes Semaphore deadlock)".to_string(),
        ));
    }

    // Validate failed_jobs_log is absolute if set
    if let Some(ref log_path) = config.pipeline.failed_jobs_log {
        if !log_path.is_absolute() {
            return Err(RagError::Config(format!(
                "pipeline.failed_jobs_log must be an absolute path (got: {}). \
                 systemd daemon runs with CWD = / and relative paths resolve there.",
                log_path.display()
            )));
        }
    }

    Ok(config)
}
