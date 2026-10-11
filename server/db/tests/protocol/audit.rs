use porchlight_db::testing::{insert_appointment, seed_household};
use porchlight_db::tx::{Actor, begin_as};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn code(pool: &PgPool, sql: &'static str) -> Option<String> {
    sqlx::query(sql).execute(pool).await.err().and_then(|e| {
        e.as_database_error()
            .and_then(|d| d.code().map(|c| c.into_owned()))
    })
}

async fn revs(pool: &PgPool, id: Uuid) -> Vec<(i32, Value, Value)> {
    sqlx::query_as(
        "select rev, snapshot, actor from porchlight.item_revisions where item_id = $1 order by rev",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn events(pool: &PgPool, id: Uuid) -> Vec<(String, Value)> {
    sqlx::query_as(
        "select kind, data from porchlight.task_events where item_id = $1 order by created_at, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn assign(pool: &PgPool, h: &porchlight_db::testing::Household, id: Uuid, members: &[Uuid]) {
    sqlx::query(
        "insert into porchlight.item_assignees (household_id, item_id, member_id) \
         select $1, $2, m from unnest($3::uuid[]) m",
    )
    .bind(h.id)
    .bind(id)
    .bind(members)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn insert_writes_revision_and_created_event(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let r = revs(&pool, id).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].0, 1);
    assert_eq!(r[0].1["title"], "Dentist");
    assert_eq!(r[0].1["assignees"], json!([]));
    let e = events(&pool, id).await;
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].0, "created");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn title_update_writes_rev_2_and_updated_event(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set title = 'Dentist 2' where id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let r = revs(&pool, id).await;
    assert_eq!(r.len(), 2);
    assert_eq!(r[1].0, 2);
    let e = events(&pool, id).await;
    assert_eq!(e.len(), 2);
    assert_eq!(e[1].0, "updated");
    assert_eq!(e[1].1, json!({"changed": ["title"]}));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn two_assignees_in_one_statement_write_one_revision(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    assign(&pool, &h, id, &[h.user, h.dependent]).await;
    let r = revs(&pool, id).await;
    assert_eq!(r.len(), 2);
    let mut want = vec![h.user.to_string(), h.dependent.to_string()];
    want.sort();
    assert_eq!(r[1].1["assignees"], json!(want));
    let e = events(&pool, id).await;
    assert_eq!(e.iter().filter(|(k, _)| k == "assigned").count(), 2);
    // Only the child bump: no spurious `updated` event.
    assert_eq!(e.len(), 3);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn deleting_an_assignee_writes_revision_and_unassigned(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    assign(&pool, &h, id, &[h.user, h.dependent]).await;
    sqlx::query("delete from porchlight.item_assignees where item_id = $1 and member_id = $2")
        .bind(id)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let r = revs(&pool, id).await;
    assert_eq!(r.len(), 3);
    assert_eq!(r[2].1["assignees"], json!([h.dependent.to_string()]));
    let e = events(&pool, id).await;
    let un: Vec<_> = e.iter().filter(|(k, _)| k == "unassigned").collect();
    assert_eq!(un.len(), 1);
    assert_eq!(un[0].1, json!({"member_id": h.user.to_string()}));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn soft_delete_and_restore_write_events(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set deleted_at = now() where id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.items set deleted_at = null where id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let kinds: Vec<String> = events(&pool, id).await.into_iter().map(|e| e.0).collect();
    assert_eq!(kinds, ["created", "deleted", "restored"]);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn rollback_leaves_no_audit_rows(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let id: Uuid = sqlx::query_scalar(
        "insert into porchlight.items (household_id, owner_id, title, kind) \
         values ($1, $2, 'Ghost', 'todo') returning id",
    )
    .bind(h.id)
    .bind(h.super_admin)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let n: i64 =
        sqlx::query_scalar("select count(*) from porchlight.item_revisions where item_id = $1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(n, 1);
    tx.rollback().await.unwrap();
    assert!(revs(&pool, id).await.is_empty());
    assert!(events(&pool, id).await.is_empty());
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn audit_tables_are_append_only(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    insert_appointment(&pool, &h).await.unwrap();
    for sql in [
        "update porchlight.item_revisions set actor = '{}'",
        "delete from porchlight.item_revisions",
        "update porchlight.task_events set actor = '{}'",
        "delete from porchlight.task_events",
    ] {
        assert_eq!(code(&pool, sql).await.as_deref(), Some("PL403"), "{sql}");
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn revision_actor_carries_member_id(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let actor = Actor {
        member_id: Some(h.user),
        device_id: None,
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let mut tx = begin_as(&pool, &actor).await.unwrap();
    let id: Uuid = sqlx::query_scalar(
        "insert into porchlight.items (household_id, owner_id, title, kind) \
         values ($1, $2, 'Mine', 'todo') returning id",
    )
    .bind(h.id)
    .bind(h.user)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let r = revs(&pool, id).await;
    assert_eq!(r[0].2["member_id"], h.user.to_string());
    let e: Value =
        sqlx::query_scalar("select actor from porchlight.task_events where item_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(e["member_id"], h.user.to_string());
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn occurrence_completion_writes_event_but_no_revision(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_occurrences (household_id, item_id, seq, status) \
         values ($1, $2, 0, 'todo')",
    )
    .bind(h.id)
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "update porchlight.item_occurrences set status = 'done', completed_at = now() where item_id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let e = events(&pool, id).await;
    let done: Vec<_> = e.iter().filter(|(k, _)| k == "completed").collect();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].1, json!({"from": "todo", "to": "done"}));
    assert_eq!(e.len(), 2);
    assert_eq!(revs(&pool, id).await.len(), 1);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn viewer_insert_writes_revision_but_no_event(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_viewers (household_id, item_id, member_id) values ($1, $2, $3)",
    )
    .bind(h.id)
    .bind(id)
    .bind(h.dependent)
    .execute(&pool)
    .await
    .unwrap();
    let r = revs(&pool, id).await;
    assert_eq!(r.len(), 2);
    assert_eq!(r[1].1["viewers"], json!([h.dependent.to_string()]));
    assert_eq!(events(&pool, id).await.len(), 1);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn relationship_rows_are_immutable(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    assign(&pool, &h, id, &[h.user]).await;
    sqlx::query(
        "insert into porchlight.item_viewers (household_id, item_id, member_id) values ($1, $2, $3)",
    )
    .bind(h.id)
    .bind(id)
    .bind(h.user)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into porchlight.item_reminders (household_id, item_id, offset_minutes) values ($1, $2, 15)",
    )
    .bind(h.id)
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    for sql in [
        "update porchlight.item_assignees set member_id = member_id",
        "update porchlight.item_viewers set member_id = member_id",
        "update porchlight.item_reminders set offset_minutes = 30",
    ] {
        assert_eq!(code(&pool, sql).await.as_deref(), Some("PL422"), "{sql}");
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn temp_tables_cannot_divert_audit_writes(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    for ddl in [
        "create temp table item_revisions (item_id uuid, household_id uuid, rev int, snapshot jsonb, actor jsonb)",
        "create temp table task_events (item_id uuid, household_id uuid, kind text, actor jsonb, data jsonb)",
        "create temp table items (id uuid, household_id uuid)",
    ] {
        sqlx::query(ddl).execute(&mut *conn).await.unwrap();
    }
    let id: Uuid = sqlx::query_scalar(
        "insert into porchlight.items (household_id, owner_id, title, kind) \
         values ($1, $2, 'Real', 'todo') returning id",
    )
    .bind(h.id)
    .bind(h.super_admin)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    assert_eq!(revs(&pool, id).await.len(), 1);
    assert_eq!(events(&pool, id).await.len(), 1);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn temp_tables_stay_empty(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("create temp table item_revisions (item_id uuid, household_id uuid, rev int, snapshot jsonb, actor jsonb)")
        .execute(&mut *conn).await.unwrap();
    sqlx::query("create temp table task_events (item_id uuid, household_id uuid, kind text, actor jsonb, data jsonb)")
        .execute(&mut *conn).await.unwrap();
    let _ = sqlx::query(
        "insert into porchlight.items (household_id, owner_id, title, kind) values ($1, $2, 'Real', 'todo')",
    )
    .bind(h.id)
    .bind(h.super_admin)
    .execute(&mut *conn)
    .await;
    for t in ["pg_temp.item_revisions", "pg_temp.task_events"] {
        let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("select count(*) from {t}")))
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(n, 0, "{t}");
    }
}

async fn occurrence(pool: &PgPool, h: &porchlight_db::testing::Household, item: Uuid) -> Uuid {
    sqlx::query_scalar(
        "insert into porchlight.item_occurrences (household_id, item_id, seq, status) \
         values ($1, $2, 0, 'todo') returning id",
    )
    .bind(h.id)
    .bind(item)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn set_status(pool: &PgPool, occ: Uuid, status: &'static str) {
    sqlx::query("update porchlight.item_occurrences set status = $2, completed_at = case when $2 = 'done' then now() end where id = $1")
        .bind(occ)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn occurrence_with_event_can_be_deleted(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let occ = occurrence(&pool, &h, id).await;
    set_status(&pool, occ, "doing").await;
    sqlx::query("delete from porchlight.item_occurrences where id = $1")
        .bind(occ)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(events(&pool, id).await.len(), 2);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn done_transitions_classify_events(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let occ = occurrence(&pool, &h, id).await;
    for s in ["done", "skipped", "done", "todo"] {
        set_status(&pool, occ, s).await;
    }
    let kinds: Vec<String> = events(&pool, id)
        .await
        .into_iter()
        .skip(1)
        .map(|e| e.0)
        .collect();
    assert_eq!(
        kinds,
        ["completed", "status_changed", "completed", "reopened"]
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn multi_item_assignee_insert_bumps_each_item_once(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let a = insert_appointment(&pool, &h).await.unwrap();
    let b = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees (household_id, item_id, member_id) \
         select $1, i, m from unnest($2::uuid[]) i, unnest($3::uuid[]) m",
    )
    .bind(h.id)
    .bind(vec![a, b])
    .bind(vec![h.user, h.dependent])
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(revs(&pool, a).await.len(), 2);
    assert_eq!(revs(&pool, b).await.len(), 2);
}
