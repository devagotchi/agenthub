use super::*;
use crate::{
    hosted::HostedBoard,
    policy::{Effect, Rule, Selector, Subject, catalog},
};

#[tokio::test]
async fn human_configuration_obeys_policy_and_shared_edits_check_every_affected_project() {
    let test = or_skip!(postgres());
    let store = &test.store;
    let a = HostedBoard::for_owner(store.clone(), LOCAL_TENANT, "owner", "alpha").unwrap();
    let b = HostedBoard::for_owner(store.clone(), LOCAL_TENANT, "owner", "beta").unwrap();
    a.initialize().await.unwrap();
    b.initialize().await.unwrap();
    let human = Actor::human("owner");
    let published = a
        .publish_workflow(&human, 1, "shared", "Shared")
        .await
        .unwrap();
    b.bind_workflow(&human, 1, "shared", published.version)
        .await
        .unwrap();
    let roles = vec!["admin".into()];
    let owner = Subject {
        id: "owner",
        human: true,
        kind: "human",
        roles: &roles,
    };
    transaction!(store, |connection| {
        store.scope(connection, LOCAL_TENANT).await?;
        for (id, action) in [
            ("workflow", "admin.workflows.*"),
            ("settings", "admin.settings.put"),
            ("project", "admin.projects.put"),
        ] {
            catalog::put_rule_in(
                connection,
                LOCAL_TENANT,
                &owner,
                &Rule {
                    id: id.into(),
                    name: id.into(),
                    subject: "human".into(),
                    action: action.into(),
                    selector: Selector {
                        projects: vec!["beta".into()],
                        labels: vec![],
                    },
                    effect: Effect::Deny,
                    priority: 200,
                    enabled: true,
                    version: 0,
                },
            )
            .await?;
        }
        Ok(())
    })
    .unwrap();
    let original = a.workflow().await.unwrap();
    let mut changed = original.clone();
    changed.name = "Changed".into();
    let before = b.configuration_state().await.unwrap();
    assert_eq!(
        a.edit_shared_workflow(&human, "shared", 1, &changed)
            .await
            .unwrap_err()
            .code,
        "policy_denied"
    );
    assert_eq!(a.workflow().await.unwrap(), original);
    assert_eq!(b.workflow().await.unwrap(), original);
    assert_eq!(
        b.configuration_state().await.unwrap().workflow_version,
        before.workflow_version
    );
    assert_eq!(
        b.put_settings(&human, &Settings::default())
            .await
            .unwrap_err()
            .code,
        "policy_denied"
    );
    assert_eq!(
        b.set_project(&human, &crate::ProjectSettings::default())
            .await
            .unwrap_err()
            .code,
        "policy_denied"
    );
    assert_eq!(
        b.detach_workflow(&human, before.workflow_version)
            .await
            .unwrap_err()
            .code,
        "policy_denied"
    );
    a.put_settings(&human, &Settings::default()).await.unwrap();
    let registered = Actor {
        id: "registered",
        human: false,
        wip_limit: None,
        policy: Some(crate::policy::Identity {
            kind: "codex".into(),
            roles,
        }),
    };
    assert_eq!(
        a.put_settings(&registered, &Settings::default())
            .await
            .unwrap_err()
            .code,
        "humans_only"
    );
}
