//! Pinned October access policy data, initialized once without restoring deletions.
use super::*;
use crate::policy::{Effect, Selector};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Presets {
    roles: Vec<RoleSeed>,
    policies: Vec<RuleSeed>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleSeed {
    id: String,
    description: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleSeed {
    name: String,
    subject: String,
    action: String,
    effect: Effect,
    priority: i64,
}

const FAMILY: &str = "devagotchi-access-s1";
const PRESETS: &str = include_str!("access-presets.json");

pub(super) async fn initialize_in(c: &mut AnyConnection, tenant: &str) -> DbResult<()> {
    // The caller holds the shared principal-policy advisory lock.
    let inserted = sqlx::query(
        "INSERT INTO board_policy_seed_marks(tenant_id,family) VALUES($1,$2)
        ON CONFLICT(tenant_id,family) DO NOTHING",
    )
    .bind(tenant)
    .bind(FAMILY)
    .execute(&mut *c)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Ok(());
    }
    let presets: Presets = decode(PRESETS)?;
    for role in presets.roles {
        sqlx::query(
            "INSERT INTO board_roles(tenant_id,id,description,version) VALUES($1,$2,$3,1)
            ON CONFLICT(tenant_id,id) DO NOTHING",
        )
        .bind(tenant)
        .bind(role.id)
        .bind(role.description)
        .execute(&mut *c)
        .await?;
    }
    // IDs bind positions in this immutable source pack. Later source deltas update the
    // specific mapped rule; they must not reorder or reseed this historical family.
    for (index, seed) in presets.policies.into_iter().enumerate() {
        let rule = Rule {
            id: format!("source-access-{:02}", index + 1),
            name: seed.name,
            subject: seed.subject,
            action: seed.action,
            selector: Selector::default(),
            effect: seed.effect,
            priority: seed.priority,
            enabled: true,
            version: 1,
        };
        rule.validate()?;
        sqlx::query(
            "INSERT INTO board_policy_rules(tenant_id,id,version,value) VALUES($1,$2,1,$3)
            ON CONFLICT(tenant_id,id) DO NOTHING",
        )
        .bind(tenant)
        .bind(&rule.id)
        .bind(json(&rule)?)
        .execute(&mut *c)
        .await?;
    }
    if roles_in(c, tenant).await?.len() > MAX_ROLES as usize
        || rules_in(c, tenant).await?.len() > MAX_RULES as usize
    {
        return Err(
            crate::invalid("source policy family exceeds the configured catalog limit").into(),
        );
    }
    Ok(())
}
