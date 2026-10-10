use chrono::{DateTime, Utc};
use porchlight_db::{
    derive, planning,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn availability_version_queue_actor_and_load_convergence(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:planner"))
        .await
        .unwrap();
    let mut tx = begin_as(&agent, &Actor::system()).await.unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','10:00')").bind(h.id).bind(h.super_admin).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let version: i32 = sqlx::query_scalar(
        "select version from porchlight.availability_profiles where member_id=$1",
    )
    .bind(h.super_admin)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(version > 0);
    let queued: bool = sqlx::query_scalar("select exists(select 1 from porchlight.member_load_jobs where member_id=$1 and requested_gen>applied_gen)").bind(h.super_admin).fetch_one(&pool).await.unwrap();
    assert!(queued);
    let item: uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,due_date,est_minutes) values($1,$2,'Long task','todo','floating','2026-10-12',90) returning id").bind(h.id).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, item, now()).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    planning::recompute_load(&mut tx, h.super_admin, now())
        .await
        .unwrap();
    let conflicts = derive::conflicts(&mut tx, item, &[h.super_admin])
        .await
        .unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].kind, "load");
    tx.commit().await.unwrap();
    let load: (i64,i64) = sqlx::query_as("select planned_minutes,available_minutes from porchlight.member_daily_load where member_id=$1 and date='2026-10-12'").bind(h.super_admin).fetch_one(&pool).await.unwrap();
    assert_eq!(load, (90, 60));
    sqlx::query("update porchlight.availability_windows set ends_local='11:00' where member_id=$1")
        .bind(h.super_admin)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    planning::recompute_load(&mut tx, h.super_admin, now())
        .await
        .unwrap();
    assert!(
        derive::conflicts(&mut tx, item, &[h.super_admin])
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();
    let actor: serde_json::Value = sqlx::query_scalar("select actor from porchlight.planning_audit where entity_type='availability' and entity_id=$1 order by created_at limit 1").bind(h.super_admin).fetch_one(&pool).await.unwrap();
    assert!(actor.to_string().contains("planner"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_availability_serializes_with_member_scope_without_lock_inversion(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','10:00')").bind(h.id).bind(h.super_admin).execute(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:planner"))
        .await
        .unwrap();
    let mut scope = begin_as(&app, &Actor::system()).await.unwrap();
    sqlx::query("select id from porchlight.household_members where id=$1 for no key update")
        .bind(h.super_admin)
        .execute(&mut *scope)
        .await
        .unwrap();
    let mut raw = begin_as(&agent, &Actor::system()).await.unwrap();
    let result = sqlx::query(
        "update porchlight.availability_windows set ends_local='11:00' where member_id=$1",
    )
    .bind(h.super_admin)
    .execute(&mut *raw)
    .await;
    assert!(
        matches!(result, Err(sqlx::Error::Database(ref e)) if e.code().as_deref()==Some("55P03")),
        "raw child writer must not bypass a retained member lock or wait holding child locks"
    );
    raw.rollback().await.unwrap();
    sqlx::query(
        "select id from porchlight.availability_windows where member_id=$1 for update nowait",
    )
    .bind(h.super_admin)
    .execute(&mut *scope)
    .await
    .unwrap();
    scope.commit().await.unwrap();
    let mut raw = begin_as(&agent, &Actor::system()).await.unwrap();
    sqlx::query("update porchlight.availability_windows set ends_local='11:00' where member_id=$1")
        .bind(h.super_admin)
        .execute(&mut *raw)
        .await
        .unwrap();
    raw.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn dst_load_uses_elapsed_minutes_and_defaults_empty(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,6,'00:00','04:00')").bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let schedule = planning::member_schedule(&mut conn, h.id, h.user)
        .await
        .unwrap();
    let loads = porchlight_core::schedule::daily_load(
        &std::collections::BTreeMap::from([("2027-03-14".parse().unwrap(), 181)]),
        &schedule.windows,
        schedule.tz,
    );
    assert_eq!(loads[0].available_minutes, 180);
    assert_eq!(loads[0].overload_minutes, 1);
    // Exercise persisted projection and whole-day conflict bounds, not just core math.
    let item: uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,due_date,est_minutes) values($1,$2,'DST task','todo','floating','2027-03-14',181) returning id").bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let result = derive::derive_item(&mut tx, item, now()).await.unwrap();
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].kind, "load");
    assert_eq!(
        result.conflicts[0].ends_at - result.conflicts[0].starts_at,
        chrono::Duration::hours(23)
    );
    tx.commit().await.unwrap();
    let load:(i64,i64) = sqlx::query_as("select planned_minutes,available_minutes from porchlight.member_daily_load where member_id=$1 and date='2027-03-14'").bind(h.user).fetch_one(&pool).await.unwrap();
    assert_eq!(load, (181, 180));
    sqlx::query("update porchlight.items set due_date='2027-11-07',est_minutes=301 where id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let result = derive::derive_item(&mut tx, item, now()).await.unwrap();
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(
        result.conflicts[0].ends_at - result.conflicts[0].starts_at,
        chrono::Duration::hours(25)
    );
    tx.commit().await.unwrap();
    let loads:Vec<(i64,i64)> = sqlx::query_as("select planned_minutes,available_minutes from porchlight.member_daily_load where member_id=$1").bind(h.user).fetch_all(&pool).await.unwrap();
    assert_eq!(loads, vec![(301, 300)]);
    assert!(
        planning::member_schedule(&mut conn, h.id, h.dependent)
            .await
            .unwrap()
            .windows
            .is_empty()
    );
}
