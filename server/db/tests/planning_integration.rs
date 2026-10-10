use chrono::{DateTime, Duration, Utc};
use porchlight_db::{
    derive, planning,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-05T13:00:00Z".parse().unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn real_load_events_are_transition_bounded_deduped_and_r47_silent(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    for days in [-1, 0, 14, 15] {
        let date = now().date_naive() + Duration::days(days);
        let item:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,due_date,est_minutes) values($1,$2,'Load event','todo','floating',$3,90) returning id").bind(h.id).bind(h.user).bind(date).fetch_one(&pool).await.unwrap();
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        derive::derive_item(&mut tx, item, now()).await.unwrap();
        tx.commit().await.unwrap();
    }
    let mut tx = begin_as(
        &app,
        &Actor {
            member_id: Some(h.user),
            ..Actor::system()
        },
    )
    .await
    .unwrap();
    planning::recompute_load(&mut tx, h.user, now())
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where kind='load_conflict'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(n, 0, "R47 empty availability is silent");
    tx.commit().await.unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) select $1,$2,d,'09:00','10:00' from generate_series(0,6) d").bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    let mut tx = begin_as(
        &app,
        &Actor {
            member_id: Some(h.user),
            ..Actor::system()
        },
    )
    .await
    .unwrap();
    planning::recompute_load(&mut tx, h.user, now())
        .await
        .unwrap();
    planning::recompute_load(&mut tx, h.user, now())
        .await
        .unwrap();
    let events:Vec<(Uuid,Option<Uuid>,serde_json::Value)>=sqlx::query_as("select recipient_member_id,actor_member_id,payload from porchlight.notification_events where kind='load_conflict' order by payload->>'date'").fetch_all(&mut *tx).await.unwrap();
    assert_eq!(
        events.len(),
        2,
        "only new dates in inclusive [today,today+14]"
    );
    for (member, actor, payload) in events {
        assert_eq!(member, h.user);
        assert_eq!(actor, Some(h.user));
        assert_eq!(payload["conflicting"], true);
    }
    tx.commit().await.unwrap();
    // Every item may have many horizon occurrences; agenda requests only a page.
    let item_ids: Vec<Uuid> =
        sqlx::query_scalar("select id from porchlight.items where household_id=$1")
            .bind(h.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    let mut conn = app.acquire().await.unwrap();
    let all = planning::load_conflicts(&mut conn, &item_ids, Some(&[h.user]))
        .await
        .unwrap();
    assert_eq!(all.len(), 4);
    let page_occurrence = all[0].1.occurrence_id;
    let page =
        planning::load_conflicts_for_occurrences(&mut conn, &item_ids, h.user, &[page_occurrence])
            .await
            .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].1.occurrence_id, page_occurrence);
    assert!(
        planning::load_conflicts_for_occurrences(&mut conn, &item_ids, h.user, &[])
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    sqlx::query("update porchlight.items set est_minutes=30 where household_id=$1")
        .bind(h.id)
        .execute(&pool)
        .await
        .unwrap();
    let items: Vec<Uuid> =
        sqlx::query_scalar("select id from porchlight.items where household_id=$1")
            .bind(h.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    for item in items {
        derive::derive_item(&mut tx, item, now()).await.unwrap();
    }
    planning::recompute_load(&mut tx, h.user, now())
        .await
        .unwrap();
    let leaves:i64=sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='load_conflict' and payload->>'conflicting'='false'").fetch_one(&mut *tx).await.unwrap();
    assert_eq!(leaves, 2, "only changed dates emit leave transitions");
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn suggestion_abi_dedupes_real_run_and_dependent_occurrence_and_suppresses_self(
    pool: PgPool,
) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let item:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'Dependent event','todo') returning id").bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, item, now()).await.unwrap();
    let occ: Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
            .bind(item)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let run = Uuid::now_v7();
    for _ in 0..2 {
        planning::emit_suggestion_notification(&mut tx, h.id, h.user, run, None, None)
            .await
            .unwrap();
        planning::emit_suggestion_notification(&mut tx, h.id, h.user, run, Some(item), Some(occ))
            .await
            .unwrap();
    }
    let n: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where kind='suggestion'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(n, 2);
    tx.commit().await.unwrap();
    let mut tx = begin_as(
        &app,
        &Actor {
            member_id: Some(h.user),
            ..Actor::system()
        },
    )
    .await
    .unwrap();
    planning::emit_suggestion_notification(&mut tx, h.id, h.user, Uuid::now_v7(), None, None)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where kind='suggestion'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(n, 2, "business self suppression comes from shared ABI");
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn leaving_authoritative_agenda_retires_proposals_but_not_history(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','17:00')").bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    for cause in ["reassignment", "subject", "kind"] {
        let item:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,due_date,est_minutes) values($1,$2,'Agenda loss','todo','floating','2026-10-12',30) returning id").bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        derive::derive_item(&mut tx, item, now()).await.unwrap();
        let (_, _, proposals) = planning::suggest(&mut tx, h.id, h.user, &[item], now())
            .await
            .unwrap();
        let old = proposals[0].clone();
        tx.commit().await.unwrap();
        for status in ["accepted", "dismissed"] {
            sqlx::query("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id,status) values($1,$2,$3,$4,$5,$6,$7,$8)").bind(h.id).bind(h.user).bind(item).bind(old.occurrence_id).bind(old.proposed_start).bind(old.proposed_end).bind(Uuid::now_v7()).bind(status).execute(&pool).await.unwrap();
        }
        match cause {
            "reassignment" => {
                sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(h.id).bind(item).bind(h.super_admin).execute(&pool).await.unwrap();
            }
            "subject" => {
                sqlx::query("update porchlight.items set subject_member_id=$2 where id=$1")
                    .bind(item)
                    .bind(h.dependent)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "kind" => {
                sqlx::query("update porchlight.items set kind='appointment',due_date=null,est_minutes=null,local_start='2026-10-12 10:00',local_end='2026-10-12 11:00' where id=$1").bind(item).execute(&pool).await.unwrap();
            }
            _ => unreachable!(),
        }
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        derive::derive_item(&mut tx, item, now()).await.unwrap();
        planning::suggest(&mut tx, h.id, h.user, &[], now())
            .await
            .unwrap();
        assert!(
            planning::suggestion(&mut tx, h.id, old.id).await.is_err(),
            "stale {cause} proposal retained"
        );
        let n:i64=sqlx::query_scalar("select count(*) from porchlight.schedule_suggestions where item_id=$1 and status in ('accepted','dismissed')").bind(item).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(n, 2);
        tx.commit().await.unwrap();
    }
}
