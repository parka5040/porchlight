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
    "2026-10-05T13:00:00Z".parse().unwrap()
}
async fn task(pool: &PgPool, household: Uuid, owner: Uuid) -> Uuid {
    sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,due_date,est_minutes) values($1,$2,'Fix2 task','todo','floating','2026-10-12',30) returning id")
        .bind(household).bind(owner).fetch_one(pool).await.unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn each_stale_cause_retires_proposed_only_and_preserves_excluded(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','17:00')").bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    for cause in [
        "completed",
        "skipped",
        "scheduled",
        "de-estimated",
        "de-deadlined",
        "expired",
        "regenerated",
        "deleted",
    ] {
        let item = task(&pool, h.id, h.user).await;
        let excluded = task(&pool, h.id, h.user).await;
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        for id in [item, excluded] {
            derive::derive_item(&mut tx, id, now()).await.unwrap();
        }
        let (_, _, proposals) = planning::suggest(&mut tx, h.id, h.user, &[item, excluded], now())
            .await
            .unwrap();
        let old = proposals
            .iter()
            .find(|s| s.item_id == item)
            .unwrap()
            .clone();
        let untouched = proposals.iter().find(|s| s.item_id == excluded).unwrap().id;
        tx.commit().await.unwrap();
        // Accepted and dismissed rows for the same stale occurrence are permanent history.
        for status in ["accepted", "dismissed"] {
            sqlx::query("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id,status) values($1,$2,$3,$4,$5,$6,$7,$8)")
                .bind(h.id).bind(h.user).bind(item).bind(old.occurrence_id).bind(old.proposed_start).bind(old.proposed_end).bind(Uuid::now_v7()).bind(status).execute(&pool).await.unwrap();
        }
        match cause {
            "completed" => {
                sqlx::query("update porchlight.item_occurrences set status='done',completed_at=$2,completed_by=$3 where id=$1").bind(old.occurrence_id).bind(now()).bind(h.user).execute(&pool).await.unwrap();
            }
            "skipped" => {
                sqlx::query("update porchlight.item_occurrences set status='skipped' where id=$1")
                    .bind(old.occurrence_id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "scheduled" => {
                sqlx::query("update porchlight.item_occurrences set override_local_start='2026-10-12T10:00',override_local_end='2026-10-12T10:30' where id=$1").bind(old.occurrence_id).execute(&pool).await.unwrap();
            }
            "de-estimated" => {
                sqlx::query("update porchlight.items set est_minutes=null where id=$1")
                    .bind(item)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "de-deadlined" => {
                sqlx::query("update porchlight.items set due_date=null where id=$1")
                    .bind(item)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "expired" => {
                sqlx::query(
                    "update porchlight.item_occurrences set disposition='expired' where id=$1",
                )
                .bind(old.occurrence_id)
                .execute(&pool)
                .await
                .unwrap();
            }
            "regenerated" => {
                sqlx::query("delete from porchlight.item_occurrences where id=$1")
                    .bind(old.occurrence_id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "deleted" => {
                sqlx::query("update porchlight.items set deleted_at=$2 where id=$1")
                    .bind(item)
                    .bind(now())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        if !matches!(cause, "expired" | "regenerated") {
            derive::derive_item(&mut tx, item, now()).await.unwrap();
        }
        // Deleted items are absent from the API agenda input, just like physical orphans.
        let allowed = if matches!(cause, "deleted") {
            vec![]
        } else {
            vec![item]
        };
        planning::suggest(&mut tx, h.id, h.user, &allowed, now())
            .await
            .unwrap();
        assert!(
            planning::suggestion(&mut tx, h.id, old.id).await.is_err(),
            "stale {cause} retained"
        );
        assert!(
            planning::suggestion(&mut tx, h.id, untouched).await.is_ok(),
            "excluded proposal lost for {cause}"
        );
        let history: i64 = sqlx::query_scalar("select count(*) from porchlight.schedule_suggestions where member_id=$1 and item_id=$2 and status in ('accepted','dismissed')").bind(h.user).bind(item).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(history, 2, "history lost for {cause}");
        tx.commit().await.unwrap();
    }
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn scope_locks_deleted_agenda_member_before_recompute(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let item = task(&pool, h.id, h.user).await;
    sqlx::query(
        "insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(h.id)
    .bind(item)
    .bind(h.dependent)
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, item, now()).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query("update porchlight.household_members set deleted_at=$2 where id=$1")
        .bind(h.dependent)
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let scope = WriteScope::begin_at(
        &mut tx,
        h.id,
        ScopeRequest {
            items: vec![item],
            ..Default::default()
        },
        now(),
    )
    .await
    .unwrap();
    let result = sqlx::query(
        "select id from porchlight.household_members where id=$1 for no key update nowait",
    )
    .bind(h.dependent)
    .execute(&pool)
    .await;
    assert!(
        matches!(result, Err(sqlx::Error::Database(ref e)) if e.code().as_deref()==Some("55P03")),
        "deleted peer member must be locked before its aggregate can be written"
    );
    scope.finish(now()).await.unwrap();
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn load_seam_called_once_per_recompute_and_untracked_members_emit_nothing(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let item = task(&pool, h.id, h.user).await;
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, item, now()).await.unwrap();
    tx.commit().await.unwrap();
    let (_, hooks) = porchlight_db::testing::with_planning_hooks(async {
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        sqlx::query("select id from porchlight.household_members where id=$1 for no key update")
            .bind(h.user)
            .execute(&mut *tx)
            .await
            .unwrap();
        planning::recompute_load(&mut tx, h.user, now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    })
    .await;
    assert_eq!(hooks.loads.len(), 1);
    assert!(hooks.loads[0].is_empty(), "R47 must emit no transitions");
    // Monday task with only Tuesday windows: Monday capacity is zero.
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,1,'09:00','10:00')").bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    let (_, hooks) = porchlight_db::testing::with_planning_hooks(async {
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        sqlx::query("select id from porchlight.household_members where id=$1 for no key update")
            .bind(h.user)
            .execute(&mut *tx)
            .await
            .unwrap();
        planning::recompute_load(&mut tx, h.user, now())
            .await
            .unwrap();
        planning::recompute_load(&mut tx, h.user, now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    })
    .await;
    assert_eq!(hooks.loads.len(), 2, "exactly one seam call per recompute");
    assert_eq!(hooks.loads[0].len(), 1);
    assert!(hooks.loads[0][0].conflicting);
    assert!(hooks.loads[1].is_empty(), "no duplicate transition");
    sqlx::query("delete from porchlight.availability_windows where member_id=$1")
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let (_, hooks) = porchlight_db::testing::with_planning_hooks(async {
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        sqlx::query("select id from porchlight.household_members where id=$1 for no key update")
            .bind(h.user)
            .execute(&mut *tx)
            .await
            .unwrap();
        planning::recompute_load(&mut tx, h.user, now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    })
    .await;
    assert_eq!(hooks.loads.len(), 1);
    assert!(
        hooks.loads[0].is_empty(),
        "disabling tracking must also stay silent"
    );
}
