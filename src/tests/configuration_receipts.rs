use super::*;
use crate::{
    ProjectSettings, SettingsEdit,
    hosted::{HostedBoard, PrincipalDirectory},
};
use devagotchi_store::Failed;
use serde_json::json;
use std::{future::Future, pin::Pin, sync::Arc};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_and_private_workflow_replay_original_results_without_reverting_later_edits() {
    let f = or_skip!(fixture());
    let actor = Actor::human("owner");
    let mut settings = f.board.settings().unwrap();
    settings.default_priority = 5;
    let put = f
        .board
        .requested("owner", "settings-put", "put_settings", &settings)
        .unwrap();
    put.put_settings(&actor, &settings).unwrap();
    settings.default_priority = 7;
    f.board.put_settings(&actor, &settings).unwrap();
    let count = f.board.events(0, 100).unwrap().len();
    settings.default_priority = 5;
    put.put_settings(&actor, &settings).unwrap();
    assert!(put.hosted().replayed());
    assert_eq!(f.board.settings().unwrap().default_priority, 7);
    assert_eq!(f.board.events(0, 100).unwrap().len(), count);
    let before = f.board.configuration_state().unwrap();
    let mut edit = SettingsEdit::from(&before.settings);
    edit.default_priority = 8;
    let change = f
        .board
        .requested(
            "owner",
            "settings-edit",
            "edit_settings",
            &json!({"version":before.settings_version,"edit":edit}),
        )
        .unwrap();
    let saved = change
        .edit_settings(&actor, before.settings_version, &edit)
        .unwrap();
    let project = ProjectSettings {
        repo: Some("https://github.com/example/one".into()),
        environments: None,
    };
    let project_call = f
        .board
        .requested("owner", "project", "set_project", &project)
        .unwrap();
    let old_project = project_call.set_project(&actor, &project).unwrap();
    f.board
        .set_project(
            &actor,
            &ProjectSettings {
                repo: Some("https://github.com/example/two".into()),
                environments: None,
            },
        )
        .unwrap();
    assert_eq!(
        change
            .edit_settings(&actor, before.settings_version, &edit)
            .unwrap(),
        saved
    );
    assert_eq!(
        project_call.set_project(&actor, &project).unwrap(),
        old_project
    );
    assert_eq!(
        f.board.settings().unwrap().repo.as_deref(),
        Some("https://github.com/example/two")
    );
    let before = f.board.configuration_state().unwrap();
    let mut workflow = before.workflow.clone();
    workflow.name = "Versioned".into();
    let edit_call = f
        .board
        .requested(
            "owner",
            "workflow-edit",
            "edit_workflow",
            &json!({"version":before.workflow_version,"workflow":workflow}),
        )
        .unwrap();
    let edited = edit_call
        .edit_workflow(&actor, before.workflow_version, &workflow)
        .unwrap();
    workflow.name = "Replacement".into();
    let replace = f
        .board
        .requested("owner", "workflow-put", "put_workflow", &workflow)
        .unwrap();
    replace.put_workflow(&actor, &workflow).unwrap();
    let replacement = workflow.clone();
    workflow.name = "Latest".into();
    f.board.put_workflow(&actor, &workflow).unwrap();
    assert_eq!(
        edit_call
            .edit_workflow(&actor, before.workflow_version, &edited.workflow)
            .unwrap(),
        edited
    );
    replace.put_workflow(&actor, &replacement).unwrap();
    assert_eq!(f.board.workflow().unwrap().name, "Latest");
}

// A domain fixture supplies a trusted owner; real transport/identity checks are covered
// by the hub's account-based and provider-based MCP receipt tests.
struct OwnerDirectory;
impl PrincipalDirectory for OwnerDirectory {
    fn enabled<'a>(
        &'a self,
        _: &'a mut sqlx::AnyConnection,
        _: &'a str,
        principal: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, Failed>> + Send + 'a>> {
        Box::pin(async move { Ok(principal == "owner") })
    }
    fn authorize_request<'a>(
        &'a self,
        _: &'a mut sqlx::AnyConnection,
        _: &'a str,
        principal: &'a str,
        actor: &'a Actor<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Failed>> + Send + 'a>> {
        Box::pin(async move {
            if principal == "owner" && actor.id == "owner" && actor.human {
                Ok(())
            } else {
                Err(crate::fail("not_granted", "fixture owner required").into())
            }
        })
    }
}

#[tokio::test]
async fn shared_workflow_receipts_recheck_original_projects_and_do_not_repeat_bindings_or_deletion()
{
    use crate::policy::{Effect, Rule, Selector, Subject, catalog};
    let test = or_skip!(postgres());
    let store = &test.store;
    let a = HostedBoard::for_owner(store.clone(), LOCAL_TENANT, "owner", "alpha")
        .unwrap()
        .with_principals(Arc::new(OwnerDirectory));
    let b = HostedBoard::for_owner(store.clone(), LOCAL_TENANT, "owner", "beta")
        .unwrap()
        .with_principals(Arc::new(OwnerDirectory));
    a.initialize().await.unwrap();
    b.initialize().await.unwrap();
    let human = Actor::human("owner");
    let publish = a
        .requested(
            "owner",
            "publish",
            "publish_workflow",
            &json!({"version":1,"id":"shared","name":"Shared"}),
        )
        .unwrap();
    let published = publish
        .publish_workflow(&human, 1, "shared", "Shared")
        .await
        .unwrap();
    assert_eq!(
        publish
            .publish_workflow(&human, 1, "shared", "Shared")
            .await
            .unwrap(),
        published
    );
    let bind = b
        .requested(
            "owner",
            "bind",
            "bind_workflow",
            &json!({"version":1,"id":"shared","catalog_version":1}),
        )
        .unwrap();
    let bound = bind.bind_workflow(&human, 1, "shared", 1).await.unwrap();
    assert_eq!(
        bind.bind_workflow(&human, 1, "shared", 1).await.unwrap(),
        bound
    );
    let mut changed = published.workflow.clone();
    changed.name = "Changed".into();
    let edit = a
        .requested(
            "owner",
            "shared-edit",
            "edit_shared_workflow",
            &json!({"version":1,"workflow":changed}),
        )
        .unwrap();
    let edited = edit
        .edit_shared_workflow(&human, "shared", 1, &changed)
        .await
        .unwrap();
    assert_eq!(
        edit.edit_shared_workflow(&human, "shared", 1, &changed)
            .await
            .unwrap(),
        edited
    );
    assert_eq!(
        bind.bind_workflow(&human, 1, "shared", 1).await.unwrap(),
        bound
    );
    assert_eq!(b.configuration_state().await.unwrap().workflow_version, 3);
    let detach = b
        .requested(
            "owner",
            "detach-beta",
            "detach_workflow",
            &json!({"version":3}),
        )
        .unwrap();
    let detached = detach.detach_workflow(&human, 3).await.unwrap();
    assert_eq!(detach.detach_workflow(&human, 3).await.unwrap(), detached);
    let roles = vec!["admin".into()];
    let owner = Subject {
        id: "owner",
        human: true,
        kind: "human",
        roles: &roles,
    };
    transaction!(store, |connection| {
        store.scope(connection, LOCAL_TENANT).await?;
        catalog::put_rule_in(
            connection,
            LOCAL_TENANT,
            &owner,
            &Rule {
                id: "deny-beta".into(),
                name: "Deny beta".into(),
                subject: "human".into(),
                action: "admin.workflows.put".into(),
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
        Ok(())
    })
    .unwrap();
    assert_eq!(
        edit.edit_shared_workflow(&human, "shared", 1, &changed)
            .await
            .unwrap_err()
            .code,
        "policy_denied",
        "original beta must still be checked after detaching it"
    );
    a.detach_workflow(&human, 3).await.unwrap();
    let delete = a
        .requested(
            "owner",
            "delete",
            "delete_shared_workflow",
            &json!({"id":"shared","version":2}),
        )
        .unwrap();
    delete
        .delete_shared_workflow(&human, "shared", 2)
        .await
        .unwrap();
    delete
        .delete_shared_workflow(&human, "shared", 2)
        .await
        .unwrap();
    assert!(delete.replayed());
    assert!(a.shared_workflows().await.unwrap().is_empty());
}
