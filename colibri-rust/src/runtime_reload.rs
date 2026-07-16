use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::AgentConfig;
use crate::model::{build_model, ModelClient};

#[derive(Clone)]
pub struct RuntimeSnapshot {
    pub config: Arc<AgentConfig>,
    pub model: Arc<Mutex<Box<dyn ModelClient>>>,
}

pub enum RuntimeReloadResult {
    Unchanged,
    Reloaded(RuntimeSnapshot),
    Rejected(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ConfigFingerprint {
    modified_ns: u128,
    len: u64,
}

pub struct PartialRuntimeReloader {
    path: PathBuf,
    fingerprint: Option<ConfigFingerprint>,
    snapshot: RuntimeSnapshot,
}

impl PartialRuntimeReloader {
    pub fn new(path: PathBuf, snapshot: RuntimeSnapshot) -> Self {
        let fingerprint = config_fingerprint(&path);
        Self {
            path,
            fingerprint,
            snapshot,
        }
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot.clone()
    }

    pub fn reload_if_changed(&mut self) -> RuntimeReloadResult {
        let fingerprint = config_fingerprint(&self.path);
        if fingerprint == self.fingerprint {
            return RuntimeReloadResult::Unchanged;
        }
        self.fingerprint = fingerprint;
        let candidate = match AgentConfig::load(Some(&self.path)) {
            Ok(config) => config,
            Err(error) => return RuntimeReloadResult::Rejected(error),
        };
        let mut next = (*self.snapshot.config).clone();
        next.model = candidate.model;
        next.vision = candidate.vision;
        next.web_search = candidate.web_search;
        let model = match build_model(&next.model) {
            Ok(model) => Arc::new(Mutex::new(model)),
            Err(error) => return RuntimeReloadResult::Rejected(error),
        };
        let snapshot = RuntimeSnapshot {
            config: Arc::new(next),
            model,
        };
        self.snapshot = snapshot.clone();
        RuntimeReloadResult::Reloaded(snapshot)
    }
}

fn config_fingerprint(path: &Path) -> Option<ConfigFingerprint> {
    let metadata = fs::metadata(path).ok()?;
    let modified_ns = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(ConfigFingerprint {
        modified_ns,
        len: metadata.len(),
    })
}
