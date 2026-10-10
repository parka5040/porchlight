use porchlight_db::testing::{insert_appointment, seed_household};
use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn viewer_removal_requires_selected_visibility(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    for visibility in ["private", "household", "selected"] {
        let item = insert_appointment(&pool, &h).await.unwrap();
        sqlx::query("UPDATE porchlight.items SET visibility=$2 WHERE id=$1")
            .bind(item)
            .bind(visibility)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO porchlight.item_viewers(household_id,item_id,member_id) VALUES ($1,$2,$3)",
        )
        .bind(h.id)
        .bind(item)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM porchlight.item_viewers WHERE item_id=$1")
            .bind(item)
            .execute(&pool)
            .await
            .unwrap();
        let payloads: Vec<serde_json::Value> = sqlx::query_scalar("SELECT payload FROM porchlight.outbox WHERE entity_id=$1 AND kind='item.access_changed' AND payload ? 'member_id'").bind(item).fetch_all(&pool).await.unwrap();
        if visibility == "selected" {
            assert_eq!(payloads, [serde_json::json!({"member_id":h.user})]);
        } else {
            assert!(payloads.is_empty());
        }
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn set_once_rejects_misspelled_column(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql("CREATE TEMP TABLE typo_test (value integer); INSERT INTO typo_test VALUES (1); CREATE TRIGGER guard BEFORE UPDATE ON typo_test FOR EACH ROW EXECUTE FUNCTION porchlight.pl_set_once('misspelled');").execute(&mut *tx).await.unwrap();
    let error = sqlx::query("UPDATE typo_test SET value=2")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    let db = error.as_database_error().unwrap();
    assert_eq!(db.code().as_deref(), Some("PL422"));
    assert!(db.message().contains("unknown set-once column"));
}
