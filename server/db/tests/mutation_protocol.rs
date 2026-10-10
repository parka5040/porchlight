use sqlx::PgPool;
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn set_once_requires_exactly_one_existing_column(pool: PgPool) {
    for args in ["", "''", "'missing'", "'value','value'"] {
        let mut tx = pool.begin().await.unwrap();
        let ddl = format!(
            "CREATE TEMP TABLE once_test(value integer); INSERT INTO once_test VALUES (NULL); CREATE TRIGGER guard BEFORE UPDATE ON once_test FOR EACH ROW EXECUTE FUNCTION porchlight.pl_set_once({args});"
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&mut *tx)
            .await
            .unwrap();
        let err = sqlx::query("UPDATE once_test SET value=2")
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("PL422")
        );
        tx.rollback().await.unwrap();
    }
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql("CREATE TEMP TABLE once_test(value integer); INSERT INTO once_test VALUES (NULL); CREATE TRIGGER guard BEFORE UPDATE ON once_test FOR EACH ROW EXECUTE FUNCTION porchlight.pl_set_once('value');").execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE once_test SET value=2")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE once_test SET value=2")
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = sqlx::query("UPDATE once_test SET value=NULL")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("PL422")
    );
}
