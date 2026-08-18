use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    Degraded { reason: String },
    Unavailable { reason: String },
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeCapabilities {
    pub schema: String,
    pub resources: BTreeMap<String, CapabilityState>,
    pub collections: BTreeMap<String, Vec<String>>,
    pub hash: String,
}

impl RuntimeCapabilities {
    pub fn new<R, C>(schema: impl Into<String>, resources: R, collections: C) -> Self
    where
        R: IntoIterator<Item = (&'static str, CapabilityState)>,
        C: IntoIterator<Item = (&'static str, Vec<String>)>,
    {
        let schema = sanitize(schema.into());
        let resources = resources
            .into_iter()
            .map(|(name, state)| (name.to_string(), sanitize_state(state)))
            .collect();
        let collections = collections
            .into_iter()
            .map(|(name, values)| {
                let values = values
                    .into_iter()
                    .map(sanitize)
                    .filter(|value| !value.is_empty())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                (name.to_string(), values)
            })
            .collect();
        let mut capabilities = Self {
            schema,
            resources,
            collections,
            hash: String::new(),
        };
        capabilities.hash = capabilities.canonical_hash();
        capabilities
    }

    fn canonical_hash(&self) -> String {
        let canonical = serde_json::json!({
            "schema": self.schema,
            "resources": self.resources,
            "collections": self.collections,
        });
        let bytes = serde_json::to_vec(&canonical).expect("runtime capabilities are serializable");
        format!("{:x}", Sha256::digest(bytes))
    }
}

pub(crate) fn sanitize(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(256)
        .collect()
}

fn sanitize_state(state: CapabilityState) -> CapabilityState {
    match state {
        CapabilityState::Degraded { reason } => CapabilityState::Degraded {
            reason: sanitize(reason),
        },
        CapabilityState::Unavailable { reason } => CapabilityState::Unavailable {
            reason: sanitize(reason),
        },
        state => state,
    }
}
