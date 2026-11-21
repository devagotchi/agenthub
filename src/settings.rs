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
    /// Preferred heartbeat interval, capped at a third of the actual lease length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_hint_ms: Option<i64>,
    /// An absent or empty list permits any note kind; internal lease notes are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_kinds: Option<Vec<String>>,
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

/// One consistent configuration read, with independent versions for the two forms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigurationState {
    pub settings: Settings,
    pub settings_version: i64,
    pub workflow: crate::Workflow,
    pub workflow_version: i64,
}

/// Human-editable settings. Standing approvals have their own authority and lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsEdit {
    pub lease_ttl_ms: i64,
    pub lease_min_ttl_ms: i64,
    pub lease_max_ttl_ms: i64,
    pub max_lease_expiries: i64,
    pub wip_limit_default: i64,
    pub default_priority: i64,
    #[serde(default)]
    pub heartbeat_hint_ms: Option<i64>,
    #[serde(default)]
    pub note_kinds: Option<Vec<String>>,
    pub repo: Option<String>,
    pub environments: Vec<String>,
}

impl From<&Settings> for SettingsEdit {
    fn from(s: &Settings) -> Self {
        Self {
            lease_ttl_ms: s.lease_ttl_ms,
            lease_min_ttl_ms: s.lease_min_ttl_ms,
            lease_max_ttl_ms: s.lease_max_ttl_ms,
            max_lease_expiries: s.max_lease_expiries,
            wip_limit_default: s.wip_limit_default,
            default_priority: s.default_priority,
            heartbeat_hint_ms: s.heartbeat_hint_ms,
            note_kinds: s.note_kinds.clone(),
            repo: s.repo.clone(),
            environments: s.environments.clone(),
        }
    }
}

impl SettingsEdit {
    pub(crate) fn apply(&self, current: &Settings, now: i64) -> BridgeResult<Settings> {
        if self.lease_min_ttl_ms <= 0
            || self.lease_ttl_ms < self.lease_min_ttl_ms
            || self.lease_max_ttl_ms < self.lease_ttl_ms
            || now.checked_add(self.lease_max_ttl_ms).is_none()
        {
            return Err(crate::invalid(
                "lease durations require 0 < minimum <= default <= maximum, within the clock's range",
            ));
        }
        if self.max_lease_expiries < 0 || self.wip_limit_default < 0 {
            return Err(crate::invalid(
                "WIP and expiry limits cannot be negative; zero means unlimited",
            ));
        }
        let repo = self
            .repo
            .as_deref()
            .filter(|r| !r.trim().is_empty())
            .map(check_repo)
            .transpose()?;
        let environments = check_environments(&self.environments)?;
        check_note_settings(self.heartbeat_hint_ms, self.note_kinds.as_deref())?;
        Ok(Settings {
            lease_ttl_ms: self.lease_ttl_ms,
            lease_min_ttl_ms: self.lease_min_ttl_ms,
            lease_max_ttl_ms: self.lease_max_ttl_ms,
            max_lease_expiries: self.max_lease_expiries,
            wip_limit_default: self.wip_limit_default,
            default_priority: self.default_priority,
            heartbeat_hint_ms: self.heartbeat_hint_ms,
            note_kinds: self.note_kinds.clone(),
            repo,
            environments,
            standing: current.standing.clone(),
        })
    }
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
            heartbeat_hint_ms: Some(Self::DEFAULT_HEARTBEAT_HINT_MS),
            note_kinds: Some(
                [
                    "note", "handoff", "blocker", "decision", "summary", "review", "question",
                ]
                .map(String::from)
                .to_vec(),
            ),
            repo: None,
            environments: default_environments(),
            standing: Vec::new(),
        }
    }
}

pub(crate) fn check_note_settings(hint: Option<i64>, kinds: Option<&[String]>) -> BridgeResult<()> {
    if hint.is_some_and(|hint| hint < 1_000) {
        return Err(crate::invalid("heartbeat hint must be at least one second"));
    }
    if let Some(kinds) = kinds {
        if kinds.len() > 64 {
            return Err(crate::invalid("choose at most 64 note kinds"));
        }
        for (index, kind) in kinds.iter().enumerate() {
            if !crate::valid_name(kind) || kinds[..index].contains(kind) {
                return Err(crate::invalid(
                    "note kinds must be distinct lowercase names (1 to 63 characters)",
                ));
            }
        }
    }
    Ok(())
}

impl Settings {
    pub const DEFAULT_HEARTBEAT_HINT_MS: i64 = 300_000;

    pub fn heartbeat_every_ms(&self, ttl_ms: i64) -> i64 {
        self.heartbeat_hint_ms
            .unwrap_or(Self::DEFAULT_HEARTBEAT_HINT_MS)
            .min(crate::lease::beat_every(ttl_ms))
            .max(1_000)
    }

    pub(crate) fn check_note_kind(&self, kind: &str) -> BridgeResult<()> {
        if let Some(kinds) = &self.note_kinds
            && !kinds.is_empty()
            && !kinds.iter().any(|allowed| allowed == kind)
        {
            return Err(crate::invalid(format!(
                "note kind must be one of: {}",
                kinds.join(", ")
            )));
        }
        Ok(())
    }
}
