use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;

#[sqlx::test]
async fn export_snapshot_outlives_ordinary_idle_transaction(pool: PgPool) {
    // Scale only the session default; exercise the same SET LOCAL as archive production.
    let requests = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(
            (*pool.connect_options())
                .clone()
                .options([("idle_in_transaction_session_timeout", "100ms")]),
        )
        .await
        .unwrap();
    let mut ordinary = porchlight_db::tx::begin(&requests).await.unwrap();
    let mut export = porchlight_db::tx::begin(&requests).await.unwrap();
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *export)
        .await
        .unwrap();
    super::snapshot_timeout(&mut export).await.unwrap();
    let timeout: String = sqlx::query_scalar("show idle_in_transaction_session_timeout")
        .fetch_one(&mut *export)
        .await
        .unwrap();
    assert_eq!(timeout, "15min");
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        sqlx::query("select 1")
            .execute(&mut *ordinary)
            .await
            .is_err(),
        "ordinary idle transaction was not terminated"
    );
    sqlx::query("select 1").execute(&mut *export).await.unwrap();
    export.commit().await.unwrap();
    drop(ordinary); // terminated/poisoned: never reused
    let timeout: String = sqlx::query_scalar("show idle_in_transaction_session_timeout")
        .fetch_one(&requests)
        .await
        .unwrap();
    assert_eq!(
        timeout, "100ms",
        "export exception must remain transaction-local"
    );
}
