use porchlight_db::testing::{Role, connect_as};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn schema_belongs_to_owner_and_search_path_is_set(pool: PgPool) {
    let (owner, search_path): (String, String) = sqlx::query_as(
        "select n.nspowner::regrole::text, current_setting('search_path') \
         from pg_namespace n where n.nspname = 'porchlight'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owner, "porchlight_owner");
    assert!(search_path.starts_with("porchlight"), "{search_path}");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn app_and_agent_can_use_the_schema_but_not_create_in_it(pool: PgPool) {
    for role in [Role::App, Role::Agent] {
        let mut conn = connect_as(&pool, role, Some("agent:test")).await.unwrap();
        let usage: bool =
            sqlx::query_scalar("select has_schema_privilege(current_user, 'porchlight', 'USAGE')")
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert!(usage, "{role:?} must have USAGE on schema porchlight");
        let err = sqlx::query("create table porchlight.nope (x int)")
            .execute(&mut conn)
            .await
            .unwrap_err();
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn superuser_migration_of_a_foreign_database_is_refused(pool: PgPool) {
    // Migration 0001 hands a database to porchlight_owner only when it is a sqlx test database.
    let name = format!("porchlight_scratch_{}", uuid::Uuid::now_v7().simple());
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("create database {name}")))
        .execute(&pool)
        .await
        .unwrap();
    let scratch = PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone().database(&name))
        .await
        .unwrap();
    let result = porchlight_db::MIGRATOR.run(&scratch).await;
    scratch.close().await;
    let owner: String =
        sqlx::query_scalar("select datdba::regrole::text from pg_database where datname = $1")
            .bind(&name)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "drop database {name} with (force)"
    )))
    .execute(&pool)
    .await
    .unwrap();

    let err = result.expect_err("migrations ran against a non-porchlight database");
    assert!(
        err.to_string()
            .contains("run migrations as porchlight_migrator against the porchlight database"),
        "{err}"
    );
    assert_ne!(owner, "porchlight_owner");
}

#[test]
fn every_migration_switches_to_owner_and_resets() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let sql = std::fs::read_to_string(&path).unwrap();
        let set = sql.rfind("SET LOCAL ROLE porchlight_owner;");
        let reset = sql.rfind("RESET ROLE;");
        assert!(
            set.is_some(),
            "{} must SET LOCAL ROLE porchlight_owner",
            path.display()
        );
        assert!(
            reset > set,
            "{} must RESET ROLE after its last SET LOCAL ROLE",
            path.display()
        );
    }
}
