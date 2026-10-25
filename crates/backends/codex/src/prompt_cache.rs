use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use crate::client::{
    Model, Thinking,
    pricing::ServiceTier,
    responses::{RequestProfile, ToolDefinition},
};
use crate::model::run::ModelCheckpoint;
use arc_swap::{ArcSwap, ArcSwapOption};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use crate::{NanocodeError, Result};

type PrefixFingerprint = [u8; 32];
type WarmupEntry = Arc<OnceCell<()>>;
type WarmupEntries = HashMap<PrefixFingerprint, WarmupEntry>;

#[derive(Clone)]
pub(crate) struct ModelPromptCache {
    key: Arc<OnceLock<Arc<str>>>,
    seed: Arc<str>,
    shared: Option<SharedPromptCache>,
    live: Arc<ArcSwapOption<ModelCheckpoint>>,
    request: Arc<ArcSwapOption<(Thinking, ServiceTier)>>,
}

#[derive(Clone, Default)]
pub(crate) struct SharedPromptCache {
    entries: Arc<ArcSwap<WarmupEntries>>,
}

impl ModelPromptCache {
    pub(crate) fn new(
        key: Option<Arc<str>>,
        seed: Arc<str>,
        shared: Option<SharedPromptCache>,
    ) -> Self {
        Self {
            key: Arc::new(key.map_or_else(OnceLock::new, OnceLock::from)),
            seed,
            shared,
            live: Arc::default(),
            request: Arc::default(),
        }
    }

    pub(crate) fn publish_request(&self, thinking: Thinking, service_tier: ServiceTier) {
        self.request.store(Some(Arc::new((thinking, service_tier))));
    }

    pub(crate) fn request(&self) -> Option<(Thinking, ServiceTier)> {
        self.request.load_full().map(|request| *request)
    }

    pub(crate) fn publish_live(&self, checkpoint: Option<ModelCheckpoint>) {
        self.live.store(checkpoint.map(Arc::new));
    }

    pub(crate) fn live(&self) -> Option<ModelCheckpoint> {
        self.live.load().as_deref().cloned()
    }

    pub(crate) fn key(
        &self,
        model: Model,
        tools: &[ToolDefinition],
        instructions: &str,
    ) -> Result<Arc<str>> {
        if let Some(key) = self.key.get() {
            return Ok(Arc::clone(key));
        }
        let derived = Arc::from(prefix_key(model, tools, instructions, &self.seed)?);
        Ok(Arc::clone(self.key.get_or_init(|| derived)))
    }

    pub(crate) const fn shared(&self) -> Option<&SharedPromptCache> {
        self.shared.as_ref()
    }
}

impl SharedPromptCache {
    pub(crate) async fn entry(
        &self,
        model: Model,
        profile: &RequestProfile,
    ) -> Result<WarmupEntry> {
        let fingerprint = prefix_fingerprint(model, profile)?;
        let mut entry = None;
        self.entries.rcu(|entries| {
            let mut entries = WarmupEntries::clone(entries);
            entry = Some(Arc::clone(
                entries.entry(fingerprint).or_insert_with(Arc::default),
            ));
            entries
        });
        Ok(entry.unwrap_or_default())
    }
}

const PREFIX_KEY_TAG: &str = "nanocode-prefix-";

const CUSTOM_MODEL_KEY_BUCKETS: u8 = 4;

pub(crate) fn is_prefix_key(key: &str) -> bool {
    key.starts_with(PREFIX_KEY_TAG)
}

fn prefix_key(
    model: Model,
    tools: &[ToolDefinition],
    instructions: &str,
    seed: &str,
) -> Result<String> {
    let encoded = serde_json::to_vec(&(model.as_str(), tools, instructions))
        .map_err(NanocodeError::SerializePromptPrefix)?;
    let digest = Sha256::digest(encoded);
    let hex = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if !model.is_custom() {
        return Ok(format!("{PREFIX_KEY_TAG}{hex}"));
    }
    let bucket = Sha256::digest(seed.as_bytes())[0] % CUSTOM_MODEL_KEY_BUCKETS;
    Ok(format!("{PREFIX_KEY_TAG}{hex}-{bucket}"))
}

fn prefix_fingerprint(model: Model, profile: &RequestProfile) -> Result<PrefixFingerprint> {
    let encoded = serde_json::to_vec(&(model, profile.prompt_cache_key(), profile.prefix()))
        .map_err(NanocodeError::SerializePromptPrefix)?;
    Ok(Sha256::digest(encoded).into())
}
