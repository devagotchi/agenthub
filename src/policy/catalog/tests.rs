use super::*;
use devagotchi_store::{or_skip, testing, transaction};

#[tokio::test]
async fn principal_catalog_versions_authority_and_defaults_hold_in_postgres() {
    let test = or_skip!(testing::postgres_with(&[&crate::migrations::MIGRATIONS]));
    let store = &test.store;
    let tenant = crate::LOCAL_TENANT;
    let roles = vec!["admin".into()];
    let owner = Subject {
        id: "owner",
        kind: "human",
        human: true,
        roles: &roles,
    };
    let agent = Subject {
        id: "agent",
        kind: "codex",
        human: false,
        roles: &roles,
    };
    transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        initialize_in(connection, tenant).await?;
        assert_eq!(roles_in(connection, tenant).await?.len(), 6);
        assert_eq!(rules_in(connection, tenant).await?.len(), 42);
        check_profile_in(connection, tenant, "codex", &["worker".into()]).await?;
        Ok(())
    })
    .unwrap();
    let role = Role {
        id: "extra".into(),
        description: "Extra role".into(),
        version: 0,
    };
    let saved = transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        put_role_in(connection, tenant, &owner, &role).await
    })
    .unwrap();
    assert_eq!(saved.version, 1);
    for subject in [&owner, &agent] {
        let failure = transaction!(store, |connection| {
            store.scope(connection, tenant).await?;
            put_role_in(connection, tenant, subject, &role).await
        })
        .unwrap_err();
        assert_eq!(
            failure.code,
            if subject.human {
                "version_conflict"
            } else {
                "humans_only"
            }
        );
    }
    let refused = transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        delete_rule_in(connection, tenant, &owner, "configure", 1).await
    })
    .unwrap_err();
    assert_eq!(refused.code, "self_lockout");
    let refused = transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        delete_role_in(connection, tenant, &owner, "reviewer", 1).await
    })
    .unwrap_err();
    assert_eq!(refused.code, "in_use");
    transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        delete_role_in(connection, tenant, &owner, "extra", 1).await?;
        delete_rule_in(connection, tenant, &owner, "notes", 1).await?;
        let mut settings = settings_in(connection, tenant).await?;
        settings.wip_limit_default = 3;
        put_settings_in(connection, tenant, &owner, &settings).await?;
        initialize_in(connection, tenant).await?;
        assert_eq!(
            rules_in(connection, tenant).await?.len(),
            41,
            "deleted rules stay deleted"
        );
        assert_eq!(settings_in(connection, tenant).await?.wip_limit_default, 3);
        Ok(())
    })
    .unwrap();
    for (kind, roles) in [
        ("unknown", vec![]),
        ("codex", vec!["missing".into()]),
        ("codex", vec!["worker".into(), "worker".into()]),
    ] {
        assert!(
            transaction!(store, |connection| {
                store.scope(connection, tenant).await?;
                check_profile_in(connection, tenant, kind, &roles).await
            })
            .is_err()
        );
    }
    sqlx::query("UPDATE board_policy_rules SET value = '{\"selector\":null}' WHERE tenant_id=$1 AND id='claim-ready'")
        .bind(tenant).execute(store.admin()).await.unwrap();
    let failure = transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        authorize_in(
            connection,
            tenant,
            &owner,
            "admin.roles.put",
            &Resource::default(),
        )
        .await
    })
    .unwrap_err();
    assert_eq!(failure.code, "board_damaged");
}
