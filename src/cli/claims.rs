//! The claims this OS user holds, cached per actor so a later command can pass the fence.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::write_json;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ClaimCache {
    #[serde(default)]
    pub(super) claims: BTreeMap<String, CachedClaim>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CachedClaim {
    pub(super) fence: i64,
    pub(super) version: i64,
    pub(super) lease_expires_at: i64,
}

pub(super) fn cache_path(board_dir: &Path, actor: &str) -> PathBuf {
    let safe: String = actor
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    board_dir.join("claims").join(format!("{safe}.json"))
}

pub(super) fn read_cache(board_dir: &Path, actor: &str) -> ClaimCache {
    std::fs::read_to_string(cache_path(board_dir, actor))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub(super) fn write_cache(board_dir: &Path, actor: &str, cache: &ClaimCache) {
    let path = cache_path(board_dir, actor);
    if cache.claims.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = write_json(&path, cache);
}
