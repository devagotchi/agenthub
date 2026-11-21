use super::*;
use devagotchi_store::{or_skip, testing, transaction};

#[tokio::test]
async fn access_policy_source_defaults_keep_human_decisions_and_indefinite_denials() {
    let test = or_skip!(testing::postgres_with(&[&crate::migrations::MIGRATIONS]));
    let tenant = crate::LOCAL_TENANT;
    let rules = transaction!(&test.store, |c| {
        test.store.scope(c, tenant).await?;
        initialize_in(c, tenant).await?;
        rules_in(c, tenant).await
    })
    .unwrap();
    let admin = vec!["admin".into()];
    let worker = vec!["worker".into()];
    let person = Subject {
        id: "owner",
        kind: "human",
        human: true,
        roles: &admin,
    };
    let agent = Subject {
        id: "agent",
        kind: "codex",
        human: false,
        roles: &worker,
    };
    for action in [
        "machine.enroll",
        "grant.approve",
        "grant.revoke",
        "preset.default",
        "policy.unsandboxed.enable",
        "guide.publish",
        "secret.put",
        "subscription.put",
        "role_binding.put",
        "catalog.capabilities.put",
        "gui.view",
        "resume.continue",
    ] {
        assert!(
            decide(&rules, &person, action, &Resource::default()).allowed,
            "{action}"
        );
        assert!(
            !decide(&rules, &agent, action, &Resource::default()).allowed,
            "{action}"
        );
    }
    for action in [
        "access.request",
        "exec",
        "fs.read",
        "fs.write",
        "proc.start",
        "net.egress",
        "gui.observe",
        "gui.input",
        "gui.window",
        "gui.app",
        "machine.list",
        "guide.run",
        "grant.indefinite",
    ] {
        assert!(
            decide(&rules, &agent, action, &Resource::default()).allowed,
            "{action}"
        );
    }
    for kind in ["cli", "service"] {
        let constrained = Subject { kind, ..agent };
        assert!(
            !decide(
                &rules,
                &constrained,
                "grant.indefinite",
                &Resource::default()
            )
            .allowed
        );
        assert!(decide(&rules, &constrained, "access.request", &Resource::default()).allowed);
    }
}

#[tokio::test]
async fn access_policy_source_family_preserves_edits_and_deleted_roles_and_rules() {
    let test = or_skip!(testing::postgres_with(&[&crate::migrations::MIGRATIONS]));
    let tenant = crate::LOCAL_TENANT;
    let admin = vec!["admin".into()];
    let person = Subject {
        id: "owner",
        kind: "human",
        human: true,
        roles: &admin,
    };
    transaction!(&test.store, |c| {
        test.store.scope(c, tenant).await?;
        initialize_in(c, tenant).await?;
        let mut rule = rules_in(c, tenant)
            .await?
            .into_iter()
            .find(|r| r.action == "access.request")
            .unwrap();
        rule.effect = super::super::Effect::Deny;
        let changed = put_rule_in(c, tenant, &person, &rule).await?;
        delete_role_in(c, tenant, &person, "builder", 1).await?;
        delete_rule_in(c, tenant, &person, "source-access-14", 1).await?;
        initialize_in(c, tenant).await?;
        assert!(!roles_in(c, tenant).await?.iter().any(|r| r.id == "builder"));
        let rules = rules_in(c, tenant).await?;
        assert!(!rules.iter().any(|r| r.id == "source-access-14"));
        assert_eq!(rules.iter().find(|r| r.id == changed.id).unwrap(), &changed);
        let marks: i64 =
            sqlx::query_scalar("SELECT count(*) FROM board_policy_seed_marks WHERE tenant_id=$1")
                .bind(tenant)
                .fetch_one(c)
                .await?;
        assert_eq!(marks, 1);
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn access_policy_source_family_is_atomic_and_preserves_existing_record_ids() {
    let test = or_skip!(testing::postgres_with(&[&crate::migrations::MIGRATIONS]));
    let tenant = crate::LOCAL_TENANT;
    // A pre-existing operator rule with the mapped ID wins over first-time seed data.
    let existing = Rule {
        id: "source-access-13".into(),
        name: "Operator request rule".into(),
        subject: "agent".into(),
        action: "access.request".into(),
        selector: Default::default(),
        effect: super::super::Effect::Deny,
        priority: 90,
        enabled: true,
        version: 7,
    };
    transaction!(&test.store, |c| {
        test.store.scope(c, tenant).await?;
        sqlx::query(
            "INSERT INTO board_policy_rules(tenant_id,id,version,value) VALUES($1,$2,7,$3)",
        )
        .bind(tenant)
        .bind(&existing.id)
        .bind(json(&existing)?)
        .execute(c)
        .await?;
        Ok(())
    })
    .unwrap();
    let failed: devagotchi_contracts::BridgeResult<()> = transaction!(&test.store, |c| {
        test.store.scope(c, tenant).await?;
        initialize_in(c, tenant).await?;
        Err(crate::invalid("force a refusal after initializing").into())
    });
    assert_eq!(failed.unwrap_err().code, "invalid");
    transaction!(&test.store, |c| {
        test.store.scope(c, tenant).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM board_policy_seed_marks WHERE tenant_id=$1")
                .bind(tenant)
                .fetch_one(&mut *c)
                .await?;
        assert_eq!(count, 0);
        assert_eq!(rules_in(c, tenant).await?, vec![existing.clone()]);
        initialize_in(c, tenant).await?;
        let rules = rules_in(c, tenant).await?;
        assert_eq!(
            rules.iter().find(|r| r.id == existing.id).unwrap(),
            &existing
        );
        assert_eq!(rules.len(), 42);
        Ok(())
    })
    .unwrap();
}
