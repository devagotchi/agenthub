use devagotchi_store::{or_skip, testing};

use super::MIGRATIONS;

#[test]
fn every_migration_is_embedded_with_the_repositorys_line_endings() {
    testing::assert_line_endings(&MIGRATIONS);
}

/// On a database of its own, with no host: every table AgentHub makes is a board table, and
/// its history is its own.
#[tokio::test]
async fn on_its_own_every_table_is_a_board_table_with_its_own_history() {
    let test = or_skip!(testing::postgres_with(&[&MIGRATIONS]));
    // As the login role: the hub's role, where the server has one, may not read the history.
    let mut connection = test.store.admin().acquire().await.unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables
         WHERE table_schema = 'devagotchi' ORDER BY 1",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert!(tables.len() >= 13, "{tables:?}");
    assert!(tables.iter().all(|t| t.starts_with("board_")), "{tables:?}");
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _agenthub_migrations ORDER BY version")
            .fetch_all(&mut *connection)
            .await
            .unwrap();
    assert_eq!(versions, vec![1, 2, 3, 4, 5, 6]);
}

#[tokio::test]
async fn configuration_version_adopts_existing_values_without_rewriting_them() {
    let url = or_skip!(testing::empty_postgres());
    testing::migrated_to(&url, &MIGRATIONS, 2).await;
    let mut connection = <sqlx::AnyConnection as sqlx::Connection>::connect(&url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO devagotchi.board_projects (tenant_id,id,source_hash,imported_at) VALUES ($1,'existing','native',1)")
        .bind(crate::LOCAL_TENANT).execute(&mut connection).await.unwrap();
    let settings = crate::Settings {
        default_priority: 9,
        ..Default::default()
    };
    let value = serde_json::to_string(&settings).unwrap();
    sqlx::query("INSERT INTO devagotchi.board_settings (tenant_id,board_id,key,value) VALUES ($1,'existing','settings',$2)")
        .bind(crate::LOCAL_TENANT).bind(&value).execute(&mut connection).await.unwrap();
    let store = devagotchi_store::Store::open(&url, &[&MIGRATIONS])
        .await
        .unwrap();
    let row: (String, i64) = sqlx::query_as("SELECT value,version FROM devagotchi.board_settings WHERE tenant_id=$1 AND board_id='existing' AND key='settings'")
        .bind(crate::LOCAL_TENANT).fetch_one(&mut connection).await.unwrap();
    assert_eq!(row, (value, 1));
    drop(store);
}

#[tokio::test]
async fn shared_catalog_upgrade_keeps_existing_workflows_private_and_unchanged() {
    let url = or_skip!(testing::empty_postgres());
    testing::migrated_to(&url, &MIGRATIONS, 3).await;
    let mut connection = <sqlx::AnyConnection as sqlx::Connection>::connect(&url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO devagotchi.board_projects (tenant_id,id,source_hash,imported_at,owner_id,project_name) VALUES ($1,'before','native',1,'alice','before')")
        .bind(crate::LOCAL_TENANT).execute(&mut connection).await.unwrap();
    let mut workflow = crate::Workflow::default_workflow();
    workflow.name = "Keep my private workflow".into();
    for (key, value, version) in [
        ("workflow", serde_json::to_string(&workflow).unwrap(), 7_i64),
        (
            "settings",
            serde_json::to_string(&crate::Settings::default()).unwrap(),
            3_i64,
        ),
    ] {
        sqlx::query("INSERT INTO devagotchi.board_settings (tenant_id,board_id,key,value,version) VALUES ($1,'before',$2,$3,$4)")
            .bind(crate::LOCAL_TENANT).bind(key).bind(value).bind(version).execute(&mut connection).await.unwrap();
    }
    let store = devagotchi_store::Store::open(&url, &[&MIGRATIONS])
        .await
        .unwrap();
    let board = crate::hosted::HostedBoard::new(store, crate::LOCAL_TENANT, "before").unwrap();
    let context = board.workflow_configuration().await.unwrap();
    assert_eq!(context.configuration.workflow, workflow);
    assert_eq!(context.configuration.workflow_version, 7);
    assert_eq!(context.configuration.settings_version, 3);
    assert!(context.shared_workflow.is_none());
    assert!(board.shared_workflows().await.unwrap().is_empty());
}

/// Opened again, nothing changes: every statement finds what it would make.
#[tokio::test]
async fn run_again_the_migrations_find_everything_made() {
    let test = or_skip!(testing::postgres_with(&[&MIGRATIONS]));
    drop(test.store);
    let mut connection = <sqlx::AnyConnection as sqlx::Connection>::connect(&test.url)
        .await
        .unwrap();
    for migration in MIGRATIONS.iter() {
        sqlx::raw_sql(sqlx::AssertSqlSafe(migration.sql.as_ref().to_string()))
            .execute(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("migration {}: {error}", migration.version));
    }
}
