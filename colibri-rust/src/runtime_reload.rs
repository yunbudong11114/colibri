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
pub(crate) struct ConfigFingerprint {
    modified_ns: u128,
    len: u64,
}

pub struct PartialRuntimeReloader {
    path: PathBuf,
    fingerprint: Option<ConfigFingerprint>,
    snapshot: RuntimeSnapshot,
    mcp_fingerprint: Option<ConfigFingerprint>,
}

impl PartialRuntimeReloader {
    pub fn new(path: PathBuf, snapshot: RuntimeSnapshot) -> Self {
        let fingerprint = config_fingerprint(&path);
        Self {
            path,
            fingerprint,
            mcp_fingerprint: config_fingerprint(&snapshot.config.mcp_config_path),
            snapshot,
        }
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot.clone()
    }

    pub fn reload_if_changed(&mut self) -> RuntimeReloadResult {
        let fingerprint = config_fingerprint(&self.path);
        let mcp_fingerprint = config_fingerprint(&self.snapshot.config.mcp_config_path);
        if fingerprint == self.fingerprint && mcp_fingerprint == self.mcp_fingerprint {
            return RuntimeReloadResult::Unchanged;
        }
        self.fingerprint = fingerprint;
        self.mcp_fingerprint = mcp_fingerprint;
        let candidate = match AgentConfig::load(Some(&self.path)) {
            Ok(config) => config,
            Err(error) => return RuntimeReloadResult::Rejected(error),
        };
        if let Err(error) = crate::memory::initialize(&candidate) {
            return RuntimeReloadResult::Rejected(error);
        }
        let model = match build_model(&candidate.model) {
            Ok(model) => Arc::new(Mutex::new(model)),
            Err(error) => return RuntimeReloadResult::Rejected(error),
        };
        let snapshot = RuntimeSnapshot {
            config: Arc::new(candidate),
            model,
        };
        self.fingerprint = config_fingerprint(&self.path);
        self.mcp_fingerprint = config_fingerprint(&snapshot.config.mcp_config_path);
        self.snapshot = snapshot.clone();
        RuntimeReloadResult::Reloaded(snapshot)
    }
}

pub(crate) fn config_fingerprint(path: &Path) -> Option<ConfigFingerprint> {
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
