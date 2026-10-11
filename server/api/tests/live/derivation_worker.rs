use chrono::{DateTime, Duration, Utc};
use porchlight_api::derive_worker;
use porchlight_db::{
    derive,
    testing::{Role, connect_as, insert_appointment, pool_as, seed_household},
};
use sqlx::PgPool;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}
async fn ready(pool: &PgPool) {
    sqlx::query("update porchlight.item_derivation set retry_at=$1")
        .bind(now())
        .execute(pool)
        .await
        .unwrap();
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_writes_converge_keep_actor_history_and_repair_poison(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut a = connect_as(&pool, Role::Agent, Some("agent:one"))
        .await
        .unwrap();
    let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,due_date,recurrence_mode,rrule) values($1,$2,'Raw','todo','2026-10-12','calendar','FREQ=DAILY;COUNT=2') returning id").bind(h.id).bind(h.user).fetch_one(&mut a).await.unwrap();
    let mut b = connect_as(&pool, Role::Agent, Some("agent:two"))
        .await
        .unwrap();
    sqlx::query("update porchlight.items set title='Second' where id=$1")
        .bind(id)
        .execute(&mut b)
        .await
        .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    ready(&pool).await;
    assert_eq!(derive_worker::run_batch(&app, now()).await.unwrap(), 1);
    let actors:Vec<String>=sqlx::query_scalar("select actor->>'application_name' from porchlight.item_revisions where item_id=$1 order by created_at,id").bind(id).fetch_all(&pool).await.unwrap();
    assert!(actors.contains(&"agent:one".into()) && actors.contains(&"agent:two".into()));
    let occ: uuid::Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1 and seq=0")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("update porchlight.item_occurrences set status='done',completed_at='2026-10-12T12:00Z',board_position='9080' where id=$1").bind(occ).execute(&mut a).await.unwrap();
    ready(&pool).await;
    derive_worker::run_batch(&app, now()).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("select count(*) from porchlight.member_agenda where occurrence_id=$1")
            .bind(occ)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    sqlx::query("update porchlight.items set rrule='FREQ=DAILY;COUNT=3;COUNT=4' where id=$1")
        .bind(id)
        .execute(&mut b)
        .await
        .unwrap();
    ready(&pool).await;
    for n in 0..5 {
        derive_worker::run_batch(&app, now() + Duration::hours(n))
            .await
            .unwrap();
    }
    let (attempts, error): (i32, Option<String>) = sqlx::query_as(
        "select attempts,last_error from porchlight.item_derivation where item_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 5);
    assert!(error.unwrap().contains("rrule"));
    let terminal:i64=sqlx::query_scalar("select count(*) from porchlight.outbox where entity_id=$1 and kind='derived' and payload->>'error'='true'").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(terminal, 1);
    assert_eq!(
        derive_worker::run_batch(&app, now() + Duration::days(1))
            .await
            .unwrap(),
        0
    );
    sqlx::query("update porchlight.items set rrule='FREQ=DAILY;COUNT=3' where id=$1")
        .bind(id)
        .execute(&mut b)
        .await
        .unwrap();
    ready(&pool).await;
    derive_worker::run_batch(&app, now() + Duration::days(1))
        .await
        .unwrap();
    let state:(i64,i64,i32,Option<String>)=sqlx::query_as("select requested_gen,applied_gen,attempts,last_error from porchlight.item_derivation where item_id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(state.0, state.1);
    assert_eq!(state.2, 0);
    assert!(state.3.is_none());
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn worker_skips_busy_item_and_raw_all_day_gap_derives(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    ready(&pool).await;
    let mut held = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(id)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    assert_eq!(derive_worker::run_batch(&app, now()).await.unwrap(), 0);
    held.rollback().await.unwrap();
    assert_eq!(derive_worker::run_batch(&app, now()).await.unwrap(), 1);
    sqlx::query("update porchlight.items set tzid='America/Havana',starts_at='2026-03-08T05:00Z',ends_at='2026-03-09T04:00Z',all_day=true where id=$1").bind(id).execute(&pool).await.unwrap();
    ready(&pool).await;
    for n in 0..5 {
        derive_worker::run_batch(&app, now() + Duration::hours(n))
            .await
            .unwrap();
    }
    let error: Option<String> =
        sqlx::query_scalar("select last_error from porchlight.item_derivation where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(error.is_none());
    let mut conn = app.acquire().await.unwrap();
    assert!(
        derive::load_item(&mut conn, id)
            .await
            .unwrap()
            .spec()
            .is_ok()
    );
    let local: chrono::NaiveDateTime = sqlx::query_scalar(
        "select local_start from porchlight.item_occurrences where item_id=$1 and seq=0",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(local, "2026-03-08T00:00:00".parse().unwrap());
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn derived_tickle_is_acl_checked_while_worker_stopped_and_eviction_is_present(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    ready(&pool).await;
    derive_worker::run_batch(&app, now()).await.unwrap();
    porchlight_db::outbox::assign_stream_seqs(&app, 500)
        .await
        .unwrap();
    let visible = porchlight_db::stream::read(&app, h.id, h.user, 0)
        .await
        .unwrap();
    assert!(
        visible
            .events
            .iter()
            .any(|e| e.entity_id == id && e.kind == "derived")
    );
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:acl"))
        .await
        .unwrap();
    sqlx::query("update porchlight.items set visibility='private',buffer_after_min=10 where id=$1")
        .bind(id)
        .execute(&mut agent)
        .await
        .unwrap();
    let stale: i64 =
        sqlx::query_scalar("select count(*) from porchlight.member_agenda where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stale, 1);
    let pending: bool = sqlx::query_scalar(
        "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(pending);
    // No worker runs after ACL loss; stream uses authoritative visibility for OLD derived rows too.
    porchlight_db::outbox::assign_stream_seqs(&app, 500)
        .await
        .unwrap();
    let hidden = porchlight_db::stream::read(&app, h.id, h.user, 0)
        .await
        .unwrap();
    assert!(
        hidden
            .events
            .iter()
            .all(|e| e.entity_id != id || e.kind == "item.access_changed")
    );
    assert!(
        hidden
            .events
            .iter()
            .any(|e| e.entity_id == id && e.kind == "item.access_changed")
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_completion_successor_board_move_and_notify_wakeup(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:completion"))
        .await
        .unwrap();
    let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,recurrence_mode,completion_interval) values($1,$2,'Raw completion','todo','completion','1 day') returning id").bind(h.id).bind(h.user).fetch_one(&mut agent).await.unwrap();
    ready(&pool).await;
    derive_worker::run_batch(&app, now()).await.unwrap();
    let occ: uuid::Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1 and seq=0")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let before: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("update porchlight.item_occurrences set board_position='aa80' where id=$1")
        .bind(occ)
        .execute(&mut agent)
        .await
        .unwrap();
    let after: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(derive_worker::run_batch(&app, now()).await.unwrap(), 0);
    sqlx::query("update porchlight.item_occurrences set status='done',completed_by=$2,completed_at='2026-10-05T12:00Z' where id=$1").bind(occ).bind(h.user).execute(&mut agent).await.unwrap();
    ready(&pool).await;
    derive_worker::run_batch(&app, now()).await.unwrap();
    let seqs:Vec<i32>=sqlx::query_scalar("select seq from porchlight.item_occurrences where item_id=$1 and status not in ('done','skipped')").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(seqs, vec![1]);
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let worker = tokio::spawn(derive_worker::run(app.clone(), async {
        let _ = rx.await;
    }));
    tokio::time::timeout(std::time::Duration::from_secs(2),async{loop{let listening:bool=sqlx::query_scalar("select exists(select 1 from pg_stat_activity where datname=current_database() and application_name='porchlight-derive-listener' and state='idle' and query ilike 'listen%porchlight_derive%')").fetch_one(&pool).await.unwrap();if listening{break;}tokio::task::yield_now().await;}}).await.expect("worker LISTEN ready");
    // The next poll is five seconds away; this committed raw update must wake through NOTIFY.
    sqlx::query("update porchlight.items set completion_interval='2 days' where id=$1")
        .bind(id)
        .execute(&mut agent)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let pending: bool = sqlx::query_scalar(
                "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
            if !pending {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("NOTIFY derives before next poll");
    stop.send(()).unwrap();
    worker.await.unwrap().unwrap();
}
