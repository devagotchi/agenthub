//! Source-defined authority: highest matching priority, deny at a tie, otherwise deny.
//!
//! Owns: board_roles, board_policy_rules and board_principal_settings through catalog.
//! Serves: typed policy decisions and composable catalog APIs; host adapters expose them.
//! Settings: role/kind/rule/WIP configuration; hosts supply their administration pages.
//! Depends on: PostgreSQL store and authenticated host identity/current resource data.
//! Policies add to account/project boundaries and hard lease/human guards.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod catalog;
mod defaults;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    #[serde(default)]
    pub projects: Vec<String>,
    /// Every listed label must be present.
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub subject: String,
    pub action: String,
    #[serde(default)]
    pub selector: Selector,
    pub effect: Effect,
    pub priority: i64,
    pub enabled: bool,
    /// Zero creates; positive versions compare before update.
    pub version: i64,
}

pub struct Subject<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub human: bool,
    pub roles: &'a [String],
}

/// A verified profile snapshot supplied by the authenticating host, never tool input.
#[derive(Debug, Clone)]
pub struct Identity {
    pub kind: String,
    pub roles: Vec<String>,
}

#[derive(Default)]
pub struct Resource<'a> {
    pub project: &'a str,
    pub labels: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Decision {
    pub allowed: bool,
    /// Stable id, not a user-editable display name. None means default deny.
    pub rule: Option<String>,
}

pub fn decide(
    rules: &[Rule],
    subject: &Subject<'_>,
    action: &str,
    resource: &Resource<'_>,
) -> Decision {
    let winner = rules
        .iter()
        .filter(|rule| {
            rule.enabled
                && subject_matches(&rule.subject, subject)
                && glob(&rule.action, action)
                && selector_matches(&rule.selector, resource)
        })
        .max_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| (a.effect == Effect::Deny).cmp(&(b.effect == Effect::Deny)))
                // Stable explanation independent of database row order.
                .then_with(|| b.id.cmp(&a.id))
        });
    Decision {
        allowed: winner.is_some_and(|rule| rule.effect == Effect::Allow),
        rule: winner.map(|rule| rule.id.clone()),
    }
}

fn subject_matches(pattern: &str, subject: &Subject<'_>) -> bool {
    match pattern {
        "*" => true,
        "human" => subject.human,
        "agent" => !subject.human,
        _ => {
            if let Some(role) = pattern.strip_prefix("role:") {
                subject.roles.iter().any(|value| value == role)
            } else if let Some(id) = pattern.strip_prefix("principal:") {
                subject.id == id
            } else if let Some(kind) = pattern.strip_prefix("kind:") {
                subject.kind == kind
            } else {
                false
            }
        }
    }
}

fn selector_matches(selector: &Selector, resource: &Resource<'_>) -> bool {
    (selector.projects.is_empty()
        || selector
            .projects
            .iter()
            .any(|value| value == "*" || value == resource.project))
        && selector
            .labels
            .iter()
            .all(|value| resource.labels.contains(value))
}

/// Only `*` is special; it includes colons, arrows, slashes and Unicode.
pub fn glob(pattern: &str, value: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = value.strip_prefix(first) else {
        return false;
    };
    let mut parts = parts.peekable();
    if parts.peek().is_none() {
        return rest.is_empty();
    }
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return rest.ends_with(part);
        }
        let Some(at) = rest.find(part) else {
            return false;
        };
        rest = &rest[at + part.len()..];
    }
    false
}

impl Rule {
    pub fn validate(&self) -> devagotchi_contracts::BridgeResult<()> {
        let printable = |value: &str, max: usize| {
            !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
        };
        let subject = matches!(self.subject.as_str(), "*" | "human" | "agent")
            || ["role:", "kind:", "principal:"].iter().any(|prefix| {
                self.subject
                    .strip_prefix(prefix)
                    .is_some_and(|value| printable(value, 256))
            });
        if !crate::valid_name(&self.id)
            || !printable(&self.name, 200)
            || !subject
            || !printable(&self.action, 200)
            || self.version < 0
            || self.selector.projects.len() > 100
            || self.selector.labels.len() > 100
            || self
                .selector
                .projects
                .iter()
                .chain(&self.selector.labels)
                .any(|v| !printable(v, 256))
        {
            return Err(crate::invalid("invalid policy rule, subject or selector"));
        }
        Ok(())
    }
}
