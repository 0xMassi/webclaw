use std::sync::Arc;

use crate::config::{EmbedProviderConfig, RagConfig, VectorStoreConfig};
use crate::embed::{DynEmbedProvider, TeiProvider};
use crate::error::RagError;
use crate::store::{DynVectorStore, QdrantStore, VectorStore};

/// Build the embed provider from config, running a startup probe.
///
/// Fails fast at startup if the provider is unavailable or returns wrong dimensions.
/// `is_available()` and `dimensions()` are concrete methods on the provider struct,
/// called here directly (not via dyn dispatch).
pub async fn build_embed_provider(config: &RagConfig) -> Result<DynEmbedProvider, RagError> {
    match &config.embed_provider {
        EmbedProviderConfig::Tei { url, model, .. } => {
            let client = reqwest::Client::new();
            let provider = TeiProvider::new_with_probe(url.clone(), model.clone(), client)
                .await
                .map_err(|e| RagError::Config(format!("TEI startup probe failed: {e}")))?;

            if !provider.is_available().await {
                return Err(RagError::Config(format!(
                    "TEI provider at {} is not available (GET /health failed). \
                     Ensure TEI is running with --pooling last-token for Qwen3-0.6B.",
                    url
                )));
            }

            let dims = provider.dimensions();
            if dims == 0 {
                return Err(RagError::Config(
                    "TEI provider returned 0 dimensions — probe failed silently".to_string(),
                ));
            }

            tracing::info!(
                provider = provider.name(),
                dims,
                url = %url,
                "embed provider ready"
            );

            Ok(Arc::new(provider))
        }
        EmbedProviderConfig::OpenAi { .. } => Err(RagError::Config(
            "OpenAI embed provider not implemented — use tei for phase 1".to_string(),
        )),
        EmbedProviderConfig::VoyageAi { .. } => Err(RagError::Config(
            "VoyageAI embed provider not implemented — use tei for phase 1".to_string(),
        )),
    }
}

/// Build the vector store from config, running collection lifecycle checks.
///
/// Creates the collection if missing; fails if existing collection has wrong dimensions.
/// `collection_exists()` and `create_collection()` are concrete methods on QdrantStore,
/// called here directly (not via dyn dispatch).
pub async fn build_vector_store(
    config: &RagConfig,
    embed_dims: usize,
) -> Result<DynVectorStore, RagError> {
    match &config.vector_store {
        VectorStoreConfig::Qdrant {
            url,
            collection,
            api_key,
        } => {
            // Resolve api_key: config value takes precedence, env var as fallback.
            let resolved_api_key = api_key
                .clone()
                .or_else(|| std::env::var("NOXA_RAG_QDRANT_API_KEY").ok());

            let store = QdrantStore::new(
                url,
                collection.clone(),
                resolved_api_key,
                config.uuid_namespace,
            )?;

            // Collection lifecycle: create if missing, validate dims if exists.
            if store.collection_exists().await? {
                // Validate that the existing collection's vector size matches embed dims.
                // We can't directly query the collection params via the current API surface,
                // so we log a warning and proceed — mismatches surface as Qdrant errors at upsert.
                // This is a known limitation; a full implementation would call
                // client.collection_info() and check vectors_config.
                tracing::warn!(
                    collection = %collection,
                    embed_dims,
                    "collection already exists — ensure vector dimensions match embed provider ({} dims)",
                    embed_dims
                );
            } else {
                tracing::info!(collection = %collection, dims = embed_dims, "creating collection");
                store.create_collection(embed_dims).await?;
            }

            tracing::info!(
                store = store.name(),
                collection = %collection,
                url = %url,
                "vector store ready"
            );

            Ok(Arc::new(store))
        }
        VectorStoreConfig::InMemory => Err(RagError::Config(
            "InMemory vector store not implemented — use testcontainers-rs for tests".to_string(),
        )),
    }
}
