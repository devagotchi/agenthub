//! Mutations composed with the host's identity-reference checks and audit transaction.
use super::*;

pub async fn put_role_in(
    connection: &mut AnyConnection,
    tenant: &str,
    subject: &Subject<'_>,
    role: &Role,
) -> DbResult<Role> {
    if !crate::valid_name(&role.id) || role.description.len() > 2000 || role.version < 0 {
        return Err(crate::invalid("invalid role id, description or version").into());
    }
    admin_in(connection, tenant, subject, "admin.roles.put").await?;
    let mut saved = role.clone();
    saved.version = role.version.checked_add(1).ok_or_else(conflict)?;
    let changed = if role.version == 0 {
        if roles_in(connection, tenant).await?.len() >= MAX_ROLES as usize {
            return Err(crate::invalid("principal role limit reached").into());
        }
        sqlx::query("INSERT INTO board_roles (tenant_id,id,description,version) VALUES ($1,$2,$3,1) ON CONFLICT (tenant_id,id) DO NOTHING")
            .bind(tenant).bind(&role.id).bind(&role.description).execute(&mut *connection).await?.rows_affected()
    } else {
        sqlx::query("UPDATE board_roles SET description=$3,version=$4 WHERE tenant_id=$1 AND id=$2 AND version=$5")
            .bind(tenant).bind(&role.id).bind(&role.description).bind(saved.version).bind(role.version).execute(&mut *connection).await?.rows_affected()
    };
    if changed != 1 {
        return Err(conflict());
    }
    Ok(saved)
}

/// The host must first refuse any reference from its identity profiles in this same
/// transaction, holding lock_in. This domain also refuses active policy/actor references.
pub async fn delete_role_in(
    connection: &mut AnyConnection,
    tenant: &str,
    subject: &Subject<'_>,
    id: &str,
    version: i64,
) -> DbResult<()> {
    admin_in(connection, tenant, subject, "admin.roles.delete").await?;
    if subject.roles.iter().any(|role| role == id)
        || rules_in(connection, tenant)
            .await?
            .iter()
            .any(|rule| rule.subject == format!("role:{id}"))
    {
        return Err(crate::fail("in_use", "role is still referenced").into());
    }
    let changed =
        sqlx::query("DELETE FROM board_roles WHERE tenant_id=$1 AND id=$2 AND version=$3")
            .bind(tenant)
            .bind(id)
            .bind(version)
            .execute(connection)
            .await?
            .rows_affected();
    if changed != 1 {
        return Err(conflict());
    }
    Ok(())
}

fn can_repair(rules: &[Rule], subject: &Subject<'_>) -> DbResult<()> {
    for action in ["admin.policies.put", "admin.policies.delete"] {
        if !decide(rules, subject, action, &Resource::default()).allowed {
            return Err(crate::fail(
                "self_lockout",
                "keep permission to edit and remove policies",
            )
            .into());
        }
    }
    Ok(())
}

pub async fn put_rule_in(
    connection: &mut AnyConnection,
    tenant: &str,
    subject: &Subject<'_>,
    rule: &Rule,
) -> DbResult<Rule> {
    rule.validate()?;
    admin_in(connection, tenant, subject, "admin.policies.put").await?;
    if let Some(id) = rule.subject.strip_prefix("role:") {
        if !roles_in(connection, tenant)
            .await?
            .iter()
            .any(|role| role.id == id)
        {
            return Err(crate::invalid("policy refers to an unknown role").into());
        }
    }
    let mut saved = rule.clone();
    saved.version = rule.version.checked_add(1).ok_or_else(conflict)?;
    let mut rules = rules_in(connection, tenant).await?;
    if let Some(current) = rules.iter_mut().find(|current| current.id == rule.id) {
        if current.version != rule.version {
            return Err(conflict());
        }
        *current = saved.clone();
    } else {
        if rule.version != 0 {
            return Err(conflict());
        }
        if rules.len() >= MAX_RULES as usize {
            return Err(crate::invalid("principal policy limit reached").into());
        }
        rules.push(saved.clone());
    }
    can_repair(&rules, subject)?;
    let changed = if rule.version == 0 {
        sqlx::query("INSERT INTO board_policy_rules (tenant_id,id,version,value) VALUES ($1,$2,1,$3) ON CONFLICT (tenant_id,id) DO NOTHING")
            .bind(tenant).bind(&rule.id).bind(json(&saved)?).execute(connection).await?.rows_affected()
    } else {
        sqlx::query("UPDATE board_policy_rules SET value=$3,version=$4 WHERE tenant_id=$1 AND id=$2 AND version=$5")
            .bind(tenant).bind(&rule.id).bind(json(&saved)?).bind(saved.version).bind(rule.version).execute(connection).await?.rows_affected()
    };
    if changed != 1 {
        return Err(conflict());
    }
    Ok(saved)
}

pub async fn delete_rule_in(
    connection: &mut AnyConnection,
    tenant: &str,
    subject: &Subject<'_>,
    id: &str,
    version: i64,
) -> DbResult<()> {
    admin_in(connection, tenant, subject, "admin.policies.delete").await?;
    let mut rules = rules_in(connection, tenant).await?;
    let Some(index) = rules
        .iter()
        .position(|rule| rule.id == id && rule.version == version)
    else {
        return Err(conflict());
    };
    rules.remove(index);
    can_repair(&rules, subject)?;
    let changed =
        sqlx::query("DELETE FROM board_policy_rules WHERE tenant_id=$1 AND id=$2 AND version=$3")
            .bind(tenant)
            .bind(id)
            .bind(version)
            .execute(connection)
            .await?
            .rows_affected();
    if changed != 1 {
        return Err(conflict());
    }
    Ok(())
}

/// The host checks its live profile kind references before removing configured kinds.
pub async fn put_settings_in(
    connection: &mut AnyConnection,
    tenant: &str,
    subject: &Subject<'_>,
    settings: &PrincipalSettings,
) -> DbResult<PrincipalSettings> {
    settings.validate()?;
    admin_in(connection, tenant, subject, "admin.settings.put").await?;
    let mut saved = settings.clone();
    saved.version = settings.version.checked_add(1).ok_or_else(conflict)?;
    let changed = sqlx::query(
        "UPDATE board_principal_settings SET value=$2,version=$3 WHERE tenant_id=$1 AND version=$4",
    )
    .bind(tenant)
    .bind(json(&saved)?)
    .bind(saved.version)
    .bind(settings.version)
    .execute(connection)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(conflict());
    }
    Ok(saved)
}
