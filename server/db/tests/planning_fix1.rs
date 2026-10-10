use chrono::{DateTime, Utc};
use porchlight_db::{
    derive,
    items::{ScopeRequest, WriteScope},
    planning,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}
async fn task(pool: &PgPool, household: Uuid, owner: Uuid) -> Uuid {
    sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,due_date,est_minutes) values($1,$2,'Fix task','todo','floating','2026-10-12',90) returning id").bind(household).bind(owner).fetch_one(pool).await.unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn shared_unheld_peer_member_both_scope_orders_twenty_times(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .connect_with(app.connect_options().as_ref().clone())
        .await
        .unwrap();
    for m in [h.super_admin, h.user, h.dependent] {
        sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','10:00')").bind(h.id).bind(m).execute(&pool).await.unwrap();
    }
    let mut peers = Vec::new();
    for m in [h.super_admin, h.user] {
        let id = task(&pool, h.id, m).await;
        for member in [m, h.dependent] {
            sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(h.id).bind(id).bind(member).execute(&pool).await.unwrap();
        }
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        derive::derive_item(&mut tx, id, now()).await.unwrap();
        tx.commit().await.unwrap();
        peers.push(id);
    }
    for reverse in [false, true] {
        for n in 0..20 {
            sqlx::query("update porchlight.items set est_minutes=$2 where id=any($1)")
                .bind(&peers)
                .bind(100 + n)
                .execute(&pool)
                .await
                .unwrap();
            // A legitimate Z worker owns its member, aggregate and queue. Neither peer
            // scope is allowed to wait on/update these rows while retaining X/Y locks.
            let mut held = begin_as(&app, &Actor::system()).await.unwrap();
            sqlx::query(
                "select id from porchlight.household_members where id=$1 for no key update",
            )
            .bind(h.dependent)
            .execute(&mut *held)
            .await
            .unwrap();
            sqlx::query(
                "select member_id from porchlight.member_load_jobs where member_id=$1 for update",
            )
            .bind(h.dependent)
            .execute(&mut *held)
            .await
            .unwrap();
            sqlx::query(
                "select member_id from porchlight.member_daily_load where member_id=$1 for update",
            )
            .bind(h.dependent)
            .execute(&mut *held)
            .await
            .unwrap();
            let members = if reverse {
                [h.user, h.super_admin]
            } else {
                [h.super_admin, h.user]
            };
            let mut a = begin_as(&app, &Actor::system()).await.unwrap();
            let mut b = begin_as(&app, &Actor::system()).await.unwrap();
            let sa = WriteScope::begin_at(
                &mut a,
                h.id,
                ScopeRequest {
                    members: vec![members[0]],
                    ..Default::default()
                },
                now(),
            )
            .await
            .unwrap();
            let sb = WriteScope::begin_at(
                &mut b,
                h.id,
                ScopeRequest {
                    members: vec![members[1]],
                    ..Default::default()
                },
                now(),
            )
            .await
            .unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::join!(sa.finish(now()), sb.finish(now()))
            })
            .await
            .expect("peer scopes must not wait on unheld member Z");
            result.0.unwrap();
            result.1.unwrap();
            a.commit().await.unwrap();
            b.commit().await.unwrap();
            let signals: i64 = sqlx::query_scalar(
                "select count(*) from porchlight.member_load_signals where member_id=$1",
            )
            .bind(h.dependent)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(signals >= 2);
            planning::recompute_load(&mut held, h.dependent, now())
                .await
                .unwrap();
            held.commit().await.unwrap();
            sqlx::query("delete from porchlight.member_load_signals")
                .execute(&pool)
                .await
                .unwrap();
        }
    }
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn partial_span_rejected_and_terminal_suggestion_guarded(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let item = task(&pool, h.id, h.user).await;
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, item, now()).await.unwrap();
    tx.commit().await.unwrap();
    let occ: Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    let result=sqlx::query("update porchlight.item_occurrences set override_local_start='2026-10-12T09:00' where id=$1").bind(occ).execute(&pool).await;
    assert!(
        matches!(result,Err(sqlx::Error::Database(ref e)) if e.code().as_deref()==Some("23514"))
    );
    let id:Uuid=sqlx::query_scalar("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) values($1,$2,$3,$4,'2026-10-12T13:00Z','2026-10-12T14:00Z',$5) returning id").bind(h.id).bind(h.user).bind(item).bind(occ).bind(Uuid::now_v7()).fetch_one(&pool).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:planner"))
        .await
        .unwrap();
    let mut tx = begin_as(&agent, &Actor::system()).await.unwrap();
    sqlx::query("update porchlight.schedule_suggestions set status='accepted' where id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&agent, &Actor::system()).await.unwrap();
    let result =
        sqlx::query("update porchlight.schedule_suggestions set status='proposed' where id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await;
    assert!(
        matches!(result,Err(sqlx::Error::Database(ref e)) if e.code().as_deref()==Some("PL422"))
    );
    tx.rollback().await.unwrap();
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn limited_run_preserves_proposals_for_excluded_items(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','17:00')").bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    let first = task(&pool, h.id, h.user).await;
    let second = task(&pool, h.id, h.user).await;
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    for item in [first, second] {
        derive::derive_item(&mut tx, item, now()).await.unwrap();
    }
    let (_, _, rows) = planning::suggest(&mut tx, h.id, h.user, &[first, second], now())
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let preserved = rows.iter().find(|r| r.item_id == second).unwrap().id;
    planning::suggest(&mut tx, h.id, h.user, &[first], now())
        .await
        .unwrap();
    assert!(planning::suggestion(&mut tx, h.id, preserved).await.is_ok());
    tx.rollback().await.unwrap();
}
