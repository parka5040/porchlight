use porchlight_db::testing::{Role, connect_as, pool_as};
use porchlight_db::tx::{Actor, begin_as};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn current_actor<'e, E: sqlx::PgExecutor<'e>>(ex: E) -> serde_json::Value {
    sqlx::query_scalar("select porchlight.pl_current_actor()")
        .fetch_one(ex)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn actor_is_visible_inside_begin_as(pool: PgPool) {
    let actor = Actor {
        member_id: Some(Uuid::now_v7()),
        device_id: None,
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let mut tx = begin_as(&pool, &actor).await.unwrap();
    let a = current_actor(&mut *tx).await;
    assert_eq!(a["member_id"], actor.member_id.unwrap().to_string());
    assert_eq!(a["request_id"], actor.request_id.to_string());
    assert!(a.get("device_id").is_none());
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn actor_does_not_leak_after_commit(pool: PgPool) {
    let single = PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let actor = Actor {
        member_id: Some(Uuid::now_v7()),
        device_id: Some(Uuid::now_v7()),
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let tx = begin_as(&single, &actor).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = single.begin().await.unwrap();
    let a = current_actor(&mut *tx).await;
    assert!(a.get("member_id").is_none(), "leaked: {a}");
    assert!(a.get("device_id").is_none(), "leaked: {a}");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn unlabelled_agent_write_is_pl403(pool: PgPool) {
    let mut conn = connect_as(&pool, Role::Agent, None).await.unwrap();
    let err = sqlx::query("update porchlight.items set title = 'x'")
        .execute(&mut conn)
        .await
        .unwrap_err();
    let code = err
        .as_database_error()
        .and_then(|d| d.code().map(|c| c.into_owned()));
    assert_eq!(code.as_deref(), Some("PL403"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn db_user_is_the_session_user(pool: PgPool) {
    let expected: String = sqlx::query_scalar("select session_user::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    let a = current_actor(&pool).await;
    assert_eq!(a["db_user"], expected);
    assert_eq!(a["db_user"], "postgres");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn labelled_agent_write_succeeds(pool: PgPool) {
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    porchlight_db::testing::insert_appointment(&pool, &h)
        .await
        .unwrap();
    let mut conn = connect_as(&pool, Role::Agent, Some("agent:foo"))
        .await
        .unwrap();
    let n = sqlx::query("update porchlight.items set title = 'x'")
        .execute(&mut conn)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1);
}

async fn latest_actors(pool: &PgPool, item: Uuid) -> (serde_json::Value, serde_json::Value) {
    sqlx::query_as(
        "select (select actor from porchlight.item_revisions where item_id = $1 order by rev desc limit 1), \
                (select actor from porchlight.task_events where item_id = $1 order by created_at desc, id desc limit 1)",
    )
    .bind(item)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn agent_cannot_forge_actor_settings(pool: PgPool) {
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    let id = porchlight_db::testing::insert_appointment(&pool, &h)
        .await
        .unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:test"))
        .await
        .unwrap();
    sqlx::query(
        "select set_config('porchlight.actor_member_id', $1, false), \
         set_config('porchlight.device_id', $2, false), \
         set_config('porchlight.request_id', $3, false), \
         set_config('porchlight.agent_label', 'forged-api-label', false), \
         set_config('porchlight.token_id', $4, false)",
    )
    .bind(h.super_admin.to_string())
    .bind(Uuid::now_v7().to_string())
    .bind(Uuid::now_v7().to_string())
    .bind(Uuid::now_v7().to_string())
    .execute(&mut agent)
    .await
    .unwrap();
    sqlx::query("update porchlight.items set title = 'Forged' where id = $1")
        .bind(id)
        .execute(&mut agent)
        .await
        .unwrap();
    let want = serde_json::json!({"application_name": "agent:test", "db_user": "porchlight_agent"});
    let (revision, event) = latest_actors(&pool, id).await;
    assert_eq!(revision, want);
    assert_eq!(event, want);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn agent_clearing_its_label_mid_statement_is_pl403(pool: PgPool) {
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    let id = porchlight_db::testing::insert_appointment(&pool, &h)
        .await
        .unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:test"))
        .await
        .unwrap();
    // The statement-level label check passes before any row is touched; the label is cleared while the row is
    // updated, so only the actor captured by the audit trigger can catch it.
    let err = sqlx::query(
        "update porchlight.items set title = 'x', source = set_config('application_name', '', true) \
         where id = $1",
    )
    .bind(id)
    .execute(&mut agent)
    .await
    .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("PL403")
    );
    let (revs, title): (i64, String) = sqlx::query_as(
        "select (select count(*) from porchlight.item_revisions where item_id = $1), \
                (select title from porchlight.items where id = $1)",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((revs, title.as_str()), (1, "Dentist"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn app_actor_settings_are_recorded(pool: PgPool) {
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    let id = porchlight_db::testing::insert_appointment(&pool, &h)
        .await
        .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let actor = Actor {
        member_id: Some(h.user),
        device_id: Some(Uuid::now_v7()),
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let mut tx = begin_as(&app, &actor).await.unwrap();
    sqlx::query("update porchlight.items set title = 'By app' where id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let want = serde_json::json!({
        "member_id": h.user.to_string(),
        "device_id": actor.device_id.unwrap().to_string(),
        "request_id": actor.request_id.to_string(),
        "db_user": "porchlight_app",
    });
    let (revision, event) = latest_actors(&pool, id).await;
    assert_eq!(revision, want);
    assert_eq!(event, want);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn non_agent_session_is_unaffected_by_label_rule(pool: PgPool) {
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    porchlight_db::testing::insert_appointment(&pool, &h)
        .await
        .unwrap();
    let mut conn = connect_as(&pool, Role::App, None).await.unwrap();
    let n = sqlx::query("update porchlight.items set title = 'x'")
        .execute(&mut conn)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1);
}
