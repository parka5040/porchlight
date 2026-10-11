use porchlight_db::testing::{insert_appointment, seed_household};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn unreadable_inbound_fk_fails_as_dependency_not_privilege(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("UPDATE porchlight.items SET deleted_at=now() WHERE id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE SCHEMA hidden_dependency;
        CREATE TABLE hidden_dependency.child (parent uuid REFERENCES porchlight.items(id) ON DELETE CASCADE);
        REVOKE ALL ON SCHEMA hidden_dependency FROM PUBLIC;
        REVOKE ALL ON hidden_dependency.child FROM PUBLIC;")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO hidden_dependency.child VALUES ($1)")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    let error = sqlx::query("SELECT porchlight.pl_purge_items($1,$2)")
        .bind(h.id)
        .bind(vec![item])
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("PL409")
    );
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.items WHERE id=$1")
        .bind(item)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn child_delete_trigger_cannot_create_uncounted_cascade(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("INSERT INTO porchlight.comments(household_id,item_id,author_id,body) VALUES ($1,$2,$3,'fixture')")
        .bind(h.id).bind(item).bind(h.super_admin).execute(&pool).await.unwrap();
    sqlx::query("UPDATE porchlight.items SET deleted_at=now() WHERE id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE porchlight.late_dependency(parent uuid REFERENCES porchlight.items(id) ON DELETE CASCADE);
        ALTER TABLE porchlight.late_dependency OWNER TO porchlight_owner;
        CREATE FUNCTION public.late_dependency() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
        SET search_path=pg_catalog AS $$ BEGIN INSERT INTO porchlight.late_dependency VALUES (OLD.item_id); RETURN OLD; END $$;
        CREATE TRIGGER late_dependency AFTER DELETE ON porchlight.comments FOR EACH ROW EXECUTE FUNCTION public.late_dependency();")
        .execute(&pool).await.unwrap();
    let error = sqlx::query("SELECT porchlight.pl_purge_items($1,$2)")
        .bind(h.id)
        .bind(vec![item])
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("PL409")
    );
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM porchlight.comments WHERE item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        remaining, 1,
        "all trigger, audit and deletion work must roll back"
    );
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.purge_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(audits, 0);
}
