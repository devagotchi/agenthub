//! Composable principal policy storage.
//!
//! Owns: board_roles, board_policy_rules, board_principal_settings and board_policy_seed_marks.
//! Serves: scoped serializable catalog reads/writes and profile-reference validation.
//! Settings: versioned kinds, roles, rules and WIP default through the host's owner pages.
//! Depends on: the policy evaluator and PostgreSQL; the host authenticates the actor,
//! checks identity references and appends its audit in the same transaction.

use devagotchi_contracts::BridgeResult;
use devagotchi_store::Failed;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::AnyConnection;

use super::{Resource, Rule, Subject, decide};

mod access_seed;
#[cfg(test)]
mod access_seed_tests;
mod mutations;
#[cfg(test)]
mod tests;

pub use mutations::{delete_role_in, delete_rule_in, put_role_in, put_rule_in, put_settings_in};

type DbResult<T> = std::result::Result<T, Failed>;
const MAX_ROLES: i64 = 128;
const MAX_RULES: i64 = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub id: String,
    pub description: String,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalSettings {
    pub kinds: Vec<String>,
    pub wip_limit_default: i64,
    pub version: i64,
}

impl Default for PrincipalSettings {
    fn default() -> Self {
        Self {
            kinds: [
                "human",
                "claude-code",
                "codex",
                "cursor",
                "grok",
                "cli",
                "service",
            ]
            .map(str::to_owned)
            .into(),
            wip_limit_default: 2,
            version: 1,
        }
    }
}

impl PrincipalSettings {
    pub fn validate(&self) -> BridgeResult<()> {
        if self.version < 1
            || self.wip_limit_default < 0
            || self.kinds.len() > 64
            || self
                .kinds
                .iter()
                .enumerate()
                .any(|(i, kind)| !crate::valid_name(kind) || self.kinds[..i].contains(kind))
        {
            return Err(crate::invalid(
                "invalid principal kinds, WIP default or version",
            ));
        }
        Ok(())
    }
}

fn decode<T: DeserializeOwned>(raw: &str) -> DbResult<T> {
    serde_json::from_str(raw).map_err(|_| {
        crate::fail(
            "board_damaged",
            "invalid stored principal policy configuration",
        )
        .into()
    })
}

fn json<T: Serialize>(value: &T) -> DbResult<String> {
    serde_json::to_string(value)
        .map_err(|_| crate::invalid("principal policy cannot be encoded").into())
}

/// Serialize catalog/profile mutations in a tenant. Call before checking references.
pub async fn lock_in(connection: &mut AnyConnection, tenant: &str) -> DbResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("board_principal_policy:{tenant}"))
        .execute(connection)
        .await?;
    Ok(())
}

/// First setup per source family: subsequent calls never restore deleted or edited configuration.
/// The trusted host calls this while creating its environment/account, before exposure.
pub async fn initialize_in(connection: &mut AnyConnection, tenant: &str) -> DbResult<()> {
    lock_in(connection, tenant).await?;
    let inserted = sqlx::query("INSERT INTO board_principal_settings (tenant_id,version,value) VALUES ($1,1,$2) ON CONFLICT (tenant_id) DO NOTHING")
        .bind(tenant).bind(json(&PrincipalSettings::default())?).execute(&mut *connection).await?.rows_affected();
    if inserted == 0 {
        return access_seed::initialize_in(connection, tenant).await;
    }
    for (id, description) in [
        (
            "admin",
            "Manages principals, roles, policies, workflows and settings; may force-release",
        ),
        ("reviewer", "May claim the review lane"),
        ("worker", "Claims and works ready tasks"),
        (
            "observer",
            "Read-only role; effective permissions come from policy",
        ),
    ] {
        sqlx::query("INSERT INTO board_roles (tenant_id,id,description,version) VALUES ($1,$2,$3,1) ON CONFLICT (tenant_id,id) DO NOTHING")
            .bind(tenant).bind(id).bind(description).execute(&mut *connection).await?;
    }
    for rule in super::defaults::rules() {
        sqlx::query("INSERT INTO board_policy_rules (tenant_id,id,version,value) VALUES ($1,$2,1,$3) ON CONFLICT (tenant_id,id) DO NOTHING")
            .bind(tenant).bind(&rule.id).bind(json(&rule)?).execute(&mut *connection).await?;
    }
    access_seed::initialize_in(connection, tenant).await
}

pub async fn settings_in(
    connection: &mut AnyConnection,
    tenant: &str,
) -> DbResult<PrincipalSettings> {
    let (raw, version): (String, i64) =
        sqlx::query_as("SELECT value,version FROM board_principal_settings WHERE tenant_id=$1")
            .bind(tenant)
            .fetch_one(connection)
            .await?;
    let settings: PrincipalSettings = decode(&raw)?;
    settings
        .validate()
        .map_err(|_| crate::fail("board_damaged", "invalid stored principal settings"))?;
    if settings.version != version {
        return Err(crate::fail("board_damaged", "principal settings version mismatch").into());
    }
    Ok(settings)
}

pub async fn roles_in(connection: &mut AnyConnection, tenant: &str) -> DbResult<Vec<Role>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT id,description,version FROM board_roles WHERE tenant_id=$1 ORDER BY id LIMIT 129",
    )
    .bind(tenant)
    .fetch_all(connection)
    .await?;
    if rows.len() > MAX_ROLES as usize {
        return Err(crate::fail("board_damaged", "principal role limit exceeded").into());
    }
    if rows.iter().any(|(id, description, version)| {
        !crate::valid_name(id) || description.len() > 2000 || *version < 1
    }) {
        return Err(crate::fail("board_damaged", "invalid stored principal role").into());
    }
    Ok(rows
        .into_iter()
        .map(|(id, description, version)| Role {
            id,
            description,
            version,
        })
        .collect())
}

pub async fn rules_in(connection: &mut AnyConnection, tenant: &str) -> DbResult<Vec<Rule>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT id,value,version FROM board_policy_rules WHERE tenant_id=$1 ORDER BY id LIMIT 257",
    )
    .bind(tenant)
    .fetch_all(connection)
    .await?;
    if rows.len() > MAX_RULES as usize {
        return Err(crate::fail("board_damaged", "principal policy limit exceeded").into());
    }
    rows.into_iter()
        .map(|(id, raw, version)| {
            let rule: Rule = decode(&raw)?;
            rule.validate()
                .map_err(|_| crate::fail("board_damaged", "invalid stored policy rule"))?;
            if rule.id != id || rule.version != version {
                return Err(
                    crate::fail("board_damaged", "policy identity/version mismatch").into(),
                );
            }
            Ok(rule)
        })
        .collect()
}

/// Validate profile references inside the profile's write transaction, under the same lock
/// used by role/settings deletion. Unknown roles never silently disappear from authority.
pub async fn check_profile_in(
    connection: &mut AnyConnection,
    tenant: &str,
    kind: &str,
    roles: &[String],
) -> DbResult<()> {
    lock_in(connection, tenant).await?;
    let settings = settings_in(connection, tenant).await?;
    if !crate::valid_name(kind)
        || (!settings.kinds.is_empty() && !settings.kinds.iter().any(|allowed| allowed == kind))
        || roles.len() > MAX_ROLES as usize
        || roles
            .iter()
            .enumerate()
            .any(|(i, role)| roles[..i].contains(role))
    {
        return Err(crate::invalid("unknown principal kind or invalid role list").into());
    }
    let known = roles_in(connection, tenant).await?;
    if roles
        .iter()
        .any(|id| !known.iter().any(|role| &role.id == id))
    {
        return Err(crate::invalid("unknown principal role").into());
    }
    Ok(())
}

pub async fn authorize_in(
    connection: &mut AnyConnection,
    tenant: &str,
    subject: &Subject<'_>,
    action: &str,
    resource: &Resource<'_>,
) -> DbResult<()> {
    let decision = decide(
        &rules_in(connection, tenant).await?,
        subject,
        action,
        resource,
    );
    if !decision.allowed {
        return Err(crate::fail(
            "policy_denied",
            format!(
                "{action} refused by {}",
                decision.rule.as_deref().unwrap_or("default-deny")
            ),
        )
        .into());
    }
    Ok(())
}

async fn admin_in(
    connection: &mut AnyConnection,
    tenant: &str,
    subject: &Subject<'_>,
    action: &str,
) -> DbResult<()> {
    if !subject.human {
        return Err(crate::fail("humans_only", "only a person manages principal policy").into());
    }
    lock_in(connection, tenant).await?;
    authorize_in(connection, tenant, subject, action, &Resource::default()).await
}

fn conflict() -> Failed {
    crate::fail(
        "version_conflict",
        "principal configuration changed; reload before saving",
    )
    .into()
}
