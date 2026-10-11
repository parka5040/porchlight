use porchlight_db::{
    items::{ScopeRequest, WriteScope},
    testing::{Role, connect_as, insert_appointment, seed_household},
};
use sqlx::{Connection, PgPool};

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn occurrence_fk_insert_is_compatible_with_held_scope_parent(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let mut app = connect_as(&pool, Role::App, None).await.unwrap();
    let mut scope_tx = app.begin().await.unwrap();
    let scope = WriteScope::begin_at(
        &mut scope_tx,
        h.id,
        ScopeRequest {
            items: vec![item],
            ..Default::default()
        },
        "2026-10-06T12:00:00Z".parse().unwrap(),
    )
    .await
    .unwrap();
    let mut raw = connect_as(&pool, Role::App, None).await.unwrap();
    let mut insert = raw.begin().await.unwrap();
    // The only possible row contention is the FK's parent KEY SHARE versus the
    // scope's NKU. No relationship writes, retry, or NOTIFY commit is involved.
    sqlx::query("set local lock_timeout='100ms'")
        .execute(&mut *insert)
        .await
        .unwrap();
    let id: uuid::Uuid = sqlx::query_scalar(
        "insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0) returning id",
    )
    .bind(h.id)
    .bind(item)
    .fetch_one(&mut *insert)
    .await
    .expect("occurrence FK must acquire KEY SHARE while WriteScope holds its parent");
    assert!(!id.is_nil());
    insert.rollback().await.unwrap();
    drop(scope);
    scope_tx.rollback().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn replaying_stream_scope_migration_preserves_real_list_acl(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let list: uuid::Uuid = sqlx::query_scalar(
        "insert into porchlight.lists(household_id,owner_id,name,visibility) values($1,$2,'Selected','selected') returning id",
    )
    .bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "insert into porchlight.list_viewers(household_id,list_id,member_id) values($1,$2,$3)",
    )
    .bind(h.id)
    .bind(list)
    .bind(h.super_admin)
    .execute(&pool)
    .await
    .unwrap();
    let definition =
        "select pg_get_functiondef('porchlight.pl_list_can_read(uuid,uuid)'::regprocedure)";
    let before: String = sqlx::query_scalar(definition)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql(include_str!("../../migrations/0025_stream_scopes.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let after: String = sqlx::query_scalar(definition)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, before, "late 0025 must not overwrite the S7 ACL");
    for (member, expected) in [(h.user, true), (h.super_admin, true), (h.dependent, false)] {
        let allowed: bool = sqlx::query_scalar("select porchlight.pl_list_can_read($1,$2)")
            .bind(list)
            .bind(member)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(allowed, expected);
    }
    sqlx::query("delete from porchlight.list_viewers where list_id=$1")
        .bind(list)
        .execute(&pool)
        .await
        .unwrap();
    let allowed: bool = sqlx::query_scalar("select porchlight.pl_list_can_read($1,$2)")
        .bind(list)
        .bind(h.super_admin)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !allowed,
        "viewer removal must still revoke access after replay"
    );
}
