//! The board's settings: defaults, the repositories and environments a project names, and the checks on them.

use devagotchi_contracts::BridgeResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{approval, fail};

/// An environment must be one the board's settings name; the refusal lists them.
pub(crate) fn check_environment(
    settings: &Settings,
    environment: Option<&str>,
) -> BridgeResult<()> {
    match environment {
        Some(env) if !settings.environments.iter().any(|e| e == env) => Err(fail(
            "unknown_environment",
            format!(
                "environment {env:?} is not one of this board's: {}",
                settings.environments.join(", ")
            ),
        )),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    pub lease_ttl_ms: i64,
    pub lease_min_ttl_ms: i64,
    pub lease_max_ttl_ms: i64,
    pub max_lease_expiries: i64,
    pub wip_limit_default: i64,
    pub default_priority: i64,
    /// The project's repository URL, named once; tickets pick it up.
    #[serde(default)]
    pub repo: Option<String>,
    /// The environments a ticket's `environment` may name.
    #[serde(default = "default_environments")]
    pub environments: Vec<String>,
    /// Exact steps a person approved ahead of time on this board (`approval.rs`): an
    /// approval one of these covers is approved the moment it is asked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub standing: Vec<approval::Standing>,
}

/// What `board_settings` changes: the project's repo, its environments, or both.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environments: Option<Vec<String>>,
}

/// A project's repo: an https URL with a host, no spaces, at most 500 characters.
pub(crate) fn check_repo(repo: &str) -> BridgeResult<String> {
    let repo = repo.trim();
    let host = repo
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or_default();
    if host.is_empty()
        || (!host.contains('.') && host != "localhost")
        || repo.chars().count() > 500
        || repo.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(fail(
            "invalid_settings",
            format!(
                "repo {repo:?} is not an https URL: give one like https://github.com/you/project"
            ),
        ));
    }
    Ok(repo.to_string())
}

/// A board's environments: 1 to 20 distinct lowercase names, `[a-z0-9][a-z0-9-]{0,62}`.
pub(crate) fn check_environments(environments: &[String]) -> BridgeResult<Vec<String>> {
    let names: Vec<String> = environments.iter().map(|e| e.trim().to_string()).collect();
    if names.is_empty() || names.len() > 20 {
        return Err(fail(
            "invalid_settings",
            format!(
                "environments must be 1 to 20 names, and {} were given: give them like dev, staging, prod",
                names.len()
            ),
        ));
    }
    if let Some(bad) = names.iter().find(|n| !crate::valid_name(n)) {
        return Err(fail(
            "invalid_settings",
            format!(
                "environment {bad:?} is not a lowercase name: use a-z, 0-9 and -, starting with a letter or digit"
            ),
        ));
    }
    if let Some(twice) = names
        .iter()
        .enumerate()
        .find_map(|(i, n)| names[..i].contains(n).then_some(n))
    {
        return Err(fail(
            "invalid_settings",
            format!("environment {twice:?} is named twice: name each once"),
        ));
    }
    Ok(names)
}

pub(crate) fn default_environments() -> Vec<String> {
    ["dev", "staging", "prod"].map(String::from).to_vec()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lease_ttl_ms: 15 * 60 * 1000,
            lease_min_ttl_ms: 30 * 1000,
            lease_max_ttl_ms: 60 * 60 * 1000,
            max_lease_expiries: 3,
            wip_limit_default: 2,
            default_priority: 2,
            repo: None,
            environments: default_environments(),
            standing: Vec::new(),
        }
    }
}
