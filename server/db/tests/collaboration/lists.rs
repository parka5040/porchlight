use porchlight_db::testing::{Role, connect_as, insert_appointment, seed_household};
use sqlx::PgPool;
use uuid::Uuid;
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_lists_grants_versions_revision_actors_checks_and_cross_household(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let other = seed_household(&pool).await.unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:lists-test"))
        .await
        .unwrap();
    let list: Uuid=sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,owner_id,name,visibility) VALUES($1,$2,'Private','private') RETURNING id").bind(h.id).bind(h.user).fetch_one(&mut agent).await.unwrap();
    let entry: Uuid=sqlx::query_scalar("INSERT INTO porchlight.list_entries(household_id,list_id,label) VALUES($1,$2,'Milk') RETURNING id").bind(h.id).bind(list).fetch_one(&mut agent).await.unwrap();
    assert_eq!(
        sqlstate(
            sqlx::query("UPDATE porchlight.list_entries SET version=4 WHERE id=$1")
                .bind(entry)
                .execute(&mut agent)
                .await
                .unwrap_err()
        ),
        "PL409"
    );
    assert_eq!(
        sqlstate(
            sqlx::query("UPDATE porchlight.list_entries SET list_id=$2 WHERE id=$1")
                .bind(entry)
                .bind(Uuid::now_v7())
                .execute(&mut agent)
                .await
                .unwrap_err()
        ),
        "PL422"
    );
    assert_eq!(sqlstate(sqlx::query(
            "INSERT INTO porchlight.list_viewers(household_id,list_id,member_id) VALUES($1,$2,$3)"
        )
        .bind(h.id)
        .bind(list)
        .bind(other.user)
        .execute(&mut agent)
        .await.unwrap_err()),"23503");
    sqlx::query("INSERT INTO porchlight.list_entry_checks(household_id,entry_id,checked_by) VALUES($1,$2,$3)").bind(h.id).bind(entry).bind(h.user).execute(&mut agent).await.unwrap();
    let version: i32 =
        sqlx::query_scalar("SELECT version FROM porchlight.list_entries WHERE id=$1")
            .bind(entry)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(version, 2);
    let snapshot: serde_json::Value = sqlx::query_scalar(
        "SELECT snapshot FROM porchlight.list_entry_revisions WHERE entry_id=$1 AND rev=2",
    )
    .bind(entry)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(snapshot["checks"][0]["checked_by"], h.user.to_string());
    let list_snapshot: serde_json::Value = sqlx::query_scalar(
        "SELECT snapshot FROM porchlight.list_revisions WHERE list_id=$1 ORDER BY rev DESC LIMIT 1",
    )
    .bind(list)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        list_snapshot.get("search_document").is_none(),
        "derived list search tokens are not revision fields"
    );
    assert!(
        snapshot.get("search_document").is_none(),
        "derived entry search tokens are not revision fields"
    );
    let actor: serde_json::Value = sqlx::query_scalar(
        "SELECT actor FROM porchlight.list_entry_revisions WHERE entry_id=$1 AND rev=2",
    )
    .bind(entry)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actor["application_name"], "agent:lists-test");
    sqlx::query("DELETE FROM porchlight.list_entry_checks WHERE entry_id=$1")
        .bind(entry)
        .execute(&mut agent)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.list_entry_revisions WHERE entry_id=$1",
    )
    .bind(entry)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 3);
    assert_eq!(
        sqlstate(
            sqlx::query("DELETE FROM porchlight.lists WHERE id=$1")
                .bind(list)
                .execute(&mut agent)
                .await
                .unwrap_err()
        ),
        "42501"
    );
    assert_eq!(
        sqlstate(
            sqlx::query(
                "UPDATE porchlight.list_entry_revisions SET snapshot='{}' WHERE entry_id=$1"
            )
            .bind(entry)
            .execute(&mut agent)
            .await
            .unwrap_err()
        ),
        "42501"
    );
    let mut unnamed = connect_as(&pool, Role::Agent, None).await.unwrap();
    assert_eq!(
        sqlstate(
            sqlx::query("UPDATE porchlight.lists SET name='No label' WHERE id=$1")
                .bind(list)
                .execute(&mut unnamed)
                .await
                .unwrap_err()
        ),
        "PL403"
    );
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn list_sql_acl_evictions_and_occurrence_mode_invariants(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let persistent: Uuid=sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,owner_id,name,visibility) VALUES($1,$2,'Selected','selected') RETURNING id").bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO porchlight.list_viewers(household_id,list_id,member_id) VALUES($1,$2,$3)",
    )
    .bind(h.id)
    .bind(persistent)
    .bind(h.super_admin)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT porchlight.pl_list_can_read($1,$2)")
            .bind(persistent)
            .bind(h.super_admin)
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    sqlx::query("DELETE FROM porchlight.list_viewers WHERE list_id=$1")
        .bind(persistent)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT porchlight.pl_list_can_read($1,$2)")
            .bind(persistent)
            .bind(h.super_admin)
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    let evicted: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.outbox WHERE kind='list.changed' AND payload->>'eviction'='true' AND payload->'recipients' @> jsonb_build_array($1::text))").bind(h.super_admin).fetch_one(&pool).await.unwrap();
    assert!(evicted);
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT porchlight.pl_list_can_read($1,$2)")
            .bind(persistent)
            .bind(h.dependent)
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE porchlight.items SET rrule='FREQ=DAILY;COUNT=3',recurrence_mode='calendar' WHERE id=$1").bind(item).execute(&pool).await.unwrap();
    let per: Uuid=sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,item_id,owner_id,name,mode) VALUES($1,$2,$3,'Meds','per_occurrence') RETURNING id").bind(h.id).bind(item).bind(h.user).fetch_one(&pool).await.unwrap();
    let e: Uuid=sqlx::query_scalar("INSERT INTO porchlight.list_entries(household_id,list_id,label) VALUES($1,$2,'Refill') RETURNING id").bind(h.id).bind(per).fetch_one(&pool).await.unwrap();
    for (entity, kind) in [(per, "list.changed"), (e, "list_entry.changed")] {
        let payload:serde_json::Value=sqlx::query_scalar("SELECT payload FROM porchlight.outbox WHERE entity_id=$1 AND kind=$2 ORDER BY id DESC LIMIT 1")
            .bind(entity).bind(kind).fetch_one(&pool).await.unwrap();
        assert_eq!(payload, serde_json::json!({"list_id":per,"item_id":item}));
    }

    assert_eq!(sqlstate(sqlx::query("INSERT INTO porchlight.list_entry_checks(household_id,entry_id,checked_by) VALUES($1,$2,$3)").bind(h.id).bind(e).bind(h.user).execute(&pool).await.unwrap_err()),"PL422");
    assert_eq!(sqlstate(sqlx::query("INSERT INTO porchlight.list_entry_checks(household_id,entry_id,occurrence_id,checked_by) VALUES($1,$2,$3,$4)").bind(h.id).bind(e).bind(Uuid::now_v7()).bind(h.user).execute(&pool).await.unwrap_err()),"PL422");
    assert_eq!(sqlstate(sqlx::query(
            "INSERT INTO porchlight.list_viewers(household_id,list_id,member_id) VALUES($1,$2,$3)"
        )
        .bind(h.id)
        .bind(per)
        .bind(h.super_admin)
        .execute(&pool)
        .await.unwrap_err()),"PL422");
    // A raw relationship writer must not wait for a parent while already holding child row locks.
    sqlx::query(
        "INSERT INTO porchlight.list_viewers(household_id,list_id,member_id) VALUES($1,$2,$3)",
    )
    .bind(h.id)
    .bind(persistent)
    .bind(h.super_admin)
    .execute(&pool)
    .await
    .unwrap();
    let mut locked = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM porchlight.lists WHERE id=$1 FOR NO KEY UPDATE")
        .bind(persistent)
        .fetch_one(&mut *locked)
        .await
        .unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:lists-contention"))
        .await
        .unwrap();
    let deletion = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        sqlx::query("DELETE FROM porchlight.list_viewers WHERE list_id=$1")
            .bind(persistent)
            .execute(&mut agent),
    )
    .await
    .expect("relationship parent lock must be NOWAIT");
    assert_eq!(
        deletion
            .unwrap_err()
            .as_database_error()
            .unwrap()
            .code()
            .unwrap(),
        "55P03"
    );
    locked.rollback().await.unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname='porchlight' AND tablename LIKE 'list%'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(tables.len(), 6); // Catalog inventory, not a backup/restore proof.
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_check_delete_never_waits_for_an_already_locked_entry(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let list: Uuid = sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,owner_id,name) VALUES($1,$2,'Groceries') RETURNING id").bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    let entry: Uuid = sqlx::query_scalar("INSERT INTO porchlight.list_entries(household_id,list_id,label) VALUES($1,$2,'Milk') RETURNING id").bind(h.id).bind(list).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.list_entry_checks(household_id,entry_id,checked_by) VALUES($1,$2,$3)").bind(h.id).bind(entry).bind(h.user).execute(&pool).await.unwrap();
    let mut locked = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM porchlight.list_entries WHERE id=$1 FOR NO KEY UPDATE")
        .bind(entry)
        .fetch_one(&mut *locked)
        .await
        .unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:checks-contention"))
        .await
        .unwrap();
    let deletion = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        sqlx::query("DELETE FROM porchlight.list_entry_checks WHERE entry_id=$1")
            .bind(entry)
            .execute(&mut agent),
    )
    .await
    .expect("check deletion must reject inverted lock acquisition immediately");
    assert_eq!(
        deletion
            .unwrap_err()
            .as_database_error()
            .unwrap()
            .code()
            .unwrap(),
        "55P03"
    );
    locked.rollback().await.unwrap();
    // Rejected deletes do not erase checks, bump versions or append history.
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM porchlight.list_entry_checks WHERE entry_id=$1")
            .bind(entry)
            .fetch_one(&pool)
            .await
            .unwrap();
    let version: i32 =
        sqlx::query_scalar("SELECT version FROM porchlight.list_entries WHERE id=$1")
            .bind(entry)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((count, version), (1, 2));
}

fn sqlstate(e: sqlx::Error) -> String {
    e.as_database_error()
        .and_then(|e| e.code())
        .expect("database SQLSTATE")
        .into_owned()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_metadata_guards_and_append_only_trigger(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    for (owner, parent, mode, visibility, code) in [
        (h.dependent, None, "persistent", "household", "PL422"),
        (h.user, Some(item), "per_occurrence", "household", "PL422"),
        (h.user, Some(item), "persistent", "private", "23514"),
    ] {
        let e=sqlx::query("INSERT INTO porchlight.lists(household_id,owner_id,item_id,name,mode,visibility) VALUES($1,$2,$3,'bad',$4,$5)").bind(h.id).bind(owner).bind(parent).bind(mode).bind(visibility).execute(&pool).await.unwrap_err();
        assert_eq!(sqlstate(e), code);
    }
    let list: Uuid = sqlx::query_scalar(
        "INSERT INTO porchlight.lists(household_id,owner_id,name) VALUES($1,$2,'ok') RETURNING id",
    )
    .bind(h.id)
    .bind(h.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(sqlstate(sqlx::query("INSERT INTO porchlight.list_viewers(household_id,list_id,member_id) VALUES($1,$2,$3)").bind(h.id).bind(list).bind(h.dependent).execute(&pool).await.unwrap_err()),"PL422");
    // Owner connection bypasses role grants, so this proves the trigger rather than missing UPDATE permission.
    assert_eq!(
        sqlstate(
            sqlx::query("UPDATE porchlight.list_revisions SET snapshot='{}' WHERE list_id=$1")
                .bind(list)
                .execute(&pool)
                .await
                .unwrap_err()
        ),
        "PL403"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_sql_lineage_cycle_is_bounded_and_rejects_unrelated_occurrence(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let first = insert_appointment(&pool, &h).await.unwrap();
    let second = insert_appointment(&pool, &h).await.unwrap();
    let unrelated = insert_appointment(&pool, &h).await.unwrap();
    for (id, parent) in [(first, second), (second, first)] {
        sqlx::query("UPDATE porchlight.items SET rrule='FREQ=DAILY;COUNT=3',recurrence_mode='calendar',split_from_item_id=$2,split_from_seq=1 WHERE id=$1")
            .bind(id).bind(parent).execute(&pool).await.unwrap();
    }
    let list:Uuid=sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,item_id,owner_id,name,mode) VALUES($1,$2,$3,'cycle','per_occurrence') RETURNING id").bind(h.id).bind(first).bind(h.user).fetch_one(&pool).await.unwrap();
    let entry:Uuid=sqlx::query_scalar("INSERT INTO porchlight.list_entries(household_id,list_id,label) VALUES($1,$2,'cycle') RETURNING id").bind(h.id).bind(list).fetch_one(&pool).await.unwrap();
    let occ:Uuid=sqlx::query_scalar("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq) VALUES($1,$2,0) RETURNING id").bind(h.id).bind(unrelated).fetch_one(&pool).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET statement_timeout='500ms'")
        .execute(&mut *conn)
        .await
        .unwrap();
    let error=sqlx::query("INSERT INTO porchlight.list_entry_checks(household_id,entry_id,occurrence_id,checked_by) VALUES($1,$2,$3,$4)").bind(h.id).bind(entry).bind(occ).bind(h.user).execute(&mut *conn).await.unwrap_err();
    assert_eq!(
        sqlstate(error),
        "PL422",
        "cycle must not run until statement timeout"
    );
}
