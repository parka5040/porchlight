use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn backup_runs_grants_and_constraints(pool: PgPool) {
    let privileges: (bool, bool, bool, bool, bool) = sqlx::query_as(
        "SELECT has_table_privilege('porchlight_backup','porchlight.backup_runs','INSERT,UPDATE'), \
         has_table_privilege('porchlight_backup','porchlight.backup_runs','DELETE'), \
         has_table_privilege('porchlight_app','porchlight.backup_runs','SELECT'), \
         has_table_privilege('porchlight_app','porchlight.backup_runs','INSERT,UPDATE,DELETE'), \
         has_table_privilege('porchlight_agent','porchlight.backup_runs','SELECT')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(privileges, (true, false, true, false, true));
    let member: bool =
        sqlx::query_scalar("SELECT pg_has_role('porchlight_backup','pg_read_all_data','MEMBER')")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(member);
    sqlx::query("INSERT INTO porchlight.backup_runs(status) VALUES ('running')")
        .execute(&pool)
        .await
        .unwrap();
    for sql in [
        "INSERT INTO porchlight.backup_runs(status) VALUES ('invented')",
        "INSERT INTO porchlight.backup_runs(status) VALUES ('success')",
        "INSERT INTO porchlight.backup_runs(status,bundle_file) VALUES ('running','../escape')",
    ] {
        assert!(sqlx::query(sql).execute(&pool).await.is_err());
    }
}
