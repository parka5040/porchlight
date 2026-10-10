use chrono::{TimeZone, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
async fn retention_clock_boundaries_counts_and_repeat(pool: PgPool) {
    let now = Utc.with_ymd_and_hms(2027, 2, 1, 3, 0, 0).unwrap();
    let h: Uuid = sqlx::query_scalar(
        "insert into porchlight.households(name,tzid) values('O2','UTC') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let m: Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Dependent','dependent','#123456','UTC') returning id").bind(h).fetch_one(&pool).await.unwrap();
    // Production version guard owns created_at; disable only for fixed boundary fixtures.
    sqlx::query("alter table porchlight.notifications disable trigger aa_version_guard")
        .execute(&pool)
        .await
        .unwrap();
    for age in [90, 91] {
        let e: Uuid = sqlx::query_scalar("insert into porchlight.notification_events(household_id,recipient_member_id,kind,entity_type,entity_id,created_at) values($1,$2,'assigned','member',$2,$3) returning id").bind(h).bind(m).bind(now-chrono::Duration::days(age)).fetch_one(&pool).await.unwrap();
        let n: Uuid = sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,entity_type,entity_id,window_start,release_at,created_at) values($1,$2,'assigned','assignments','member',$2,$3,$3,$3) returning id").bind(h).bind(m).bind(now-chrono::Duration::days(age)).fetch_one(&pool).await.unwrap();
        sqlx::query("insert into porchlight.notification_sources values($1,$2)")
            .bind(n)
            .bind(e)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("insert into porchlight.notification_event_delivery(event_id,attempts,retry_at,last_error) values($1,1,now(),'constraint')").bind(e).execute(&pool).await.unwrap();
    }
    sqlx::query("alter table porchlight.notifications enable trigger aa_version_guard")
        .execute(&pool)
        .await
        .unwrap();
    // Prefix only: an old row beyond a recent row must remain until that prefix ages.
    for (seq, age) in [(1, 8), (2, 7), (3, 8)] {
        sqlx::query("insert into porchlight.outbox(household_id,kind,entity_type,entity_id,created_at,stream_seq) values($1,'member.changed','member',$2,$3,$4)").bind(h).bind(m).bind(now-chrono::Duration::days(age)).bind(seq as i64).execute(&pool).await.unwrap();
    }
    let counts: serde_json::Value = sqlx::query_scalar("select porchlight.pl_retention($1)")
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
    for table in [
        "notification_events",
        "notifications",
        "notification_sources",
        "notification_event_delivery",
    ] {
        assert_eq!(counts[table], 1, "{table}: {counts}");
    }
    assert_eq!(counts["outbox"], 1);
    let epoch: i64 = sqlx::query_scalar("select epoch from porchlight.stream_retention")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(epoch, 1);
    let next: serde_json::Value = sqlx::query_scalar("select porchlight.pl_retention($1)")
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
    for table in [
        "notification_events",
        "notifications",
        "notification_sources",
        "notification_event_delivery",
    ] {
        assert_eq!(next[table], 0, "{table}: {next}");
    }
    assert_eq!(next["outbox"], 0);
    let logs: i64 = sqlx::query_scalar("select count(*) from porchlight.purge_log where household_id=$1 and cardinality(item_ids)=0").bind(h).fetch_one(&pool).await.unwrap();
    assert_eq!(logs, 2);
}
