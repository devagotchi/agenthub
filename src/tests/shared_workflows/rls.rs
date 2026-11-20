//! Exercise the database policies without application owner/tenant WHERE clauses.
pub(super) async fn prove(store: &devagotchi_store::Store) {
    // Role creation is transactional: rollback removes this test-only role and grants.
    let role = format!("workflow_test_{}", std::process::id());
    let mut tx = store.admin().begin().await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE ROLE {role} NOLOGIN NOSUPERUSER NOBYPASSRLS;
         GRANT USAGE ON SCHEMA devagotchi TO {role};
         GRANT SELECT,INSERT ON devagotchi.board_workflows,devagotchi.board_workflow_bindings TO {role};
         SET LOCAL ROLE {role};"
    ))).execute(&mut *tx).await.unwrap();
    for (tenant, expected) in [(super::TENANT, 2_i64), (super::OTHER, 1), ("", 0)] {
        sqlx::query("SELECT set_config('devagotchi.tenant',$1,true)")
            .bind(tenant)
            .execute(&mut *tx)
            .await
            .unwrap();
        let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM devagotchi.board_workflows),(SELECT count(*) FROM devagotchi.board_workflow_bindings)")
            .fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            counts,
            (expected, expected),
            "RLS applies without application WHERE clauses"
        );
    }
    sqlx::query("SELECT set_config('devagotchi.tenant',$1,true)")
        .bind(super::TENANT)
        .execute(&mut *tx)
        .await
        .unwrap();
    let refused = sqlx::query("INSERT INTO devagotchi.board_workflows (tenant_id,owner_id,id,source_board,value) VALUES ($1,'alice','forbidden','missing','{}')")
        .bind(super::OTHER).execute(&mut *tx).await.unwrap_err();
    assert_eq!(
        refused.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();
}
