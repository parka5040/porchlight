use chrono::{DateTime, Utc};
use porchlight_api::notifier;
use porchlight_db::{
    derive,
    items::{ItemWrite, ScopeRequest, WriteScope},
    testing::{insert_appointment, seed_household},
};
use sqlx::PgPool;
use uuid::Uuid;

fn at(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

async fn produce(pool: &PgPool, now: DateTime<Utc>) {
    let mut tx = pool.begin().await.unwrap();
    notifier::reminders(&mut tx, now).await.unwrap();
    tx.commit().await.unwrap();
}

async fn split_delivery(pool: &PgPool, explicit: bool, concurrent: bool, upgrade: bool) {
    let h = seed_household(pool).await.unwrap();
    sqlx::query("update porchlight.notification_prefs set default_appointment_offsets=array[30] where member_id=$1")
        .bind(h.super_admin).execute(pool).await.unwrap();
    let item = insert_appointment(pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set starts_at='2026-10-05 12:00Z',ends_at='2026-10-05 13:00Z',rrule='FREQ=DAILY;COUNT=3',recurrence_mode='calendar' where id=$1")
        .bind(item).execute(pool).await.unwrap();
    if explicit {
        sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)")
            .bind(h.id).bind(item).execute(pool).await.unwrap();
    }
    let mut tx = pool.begin().await.unwrap();
    derive::derive_item(&mut tx, item, at("2026-10-05T10:00:00Z"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let occurrence: Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1 and seq=1")
            .bind(item)
            .fetch_one(pool)
            .await
            .unwrap();
    let fired_at = at("2026-10-06T11:31:00Z");
    if !concurrent {
        produce(pool, fired_at).await;
        // Retire even the emitted projection before a backward clock correction.
        let mut tx = pool.begin().await.unwrap();
        derive::derive_item(&mut tx, item, fired_at).await.unwrap();
        tx.commit().await.unwrap();
    }
    if upgrade {
        let before: (Uuid, i32) = sqlx::query_as("select r.id,i.version from porchlight.items i cross join lateral porchlight.pl_reminder_offsets(i.id,$2) r where i.id=$1")
            .bind(item).bind(h.super_admin).fetch_one(pool).await.unwrap();
        // Bootstrap changes the database default search_path for new connections;
        // use the same ledger schema as the initial SQLx migration connection.
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("set search_path=public")
            .execute(&mut *conn)
            .await
            .unwrap();
        // Remove the current-code compatibility expression before its real migration.
        sqlx::query("drop function porchlight.pl_effective_member_zone(text,text,text)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("alter table porchlight.member_agenda drop column disposition")
            .execute(&mut *conn)
            .await
            .unwrap();
        porchlight_db::MIGRATOR.run(&mut *conn).await.unwrap();
        drop(conn);
        let after: (Uuid, i32) = sqlx::query_as("select r.id,i.version from porchlight.items i cross join lateral porchlight.pl_reminder_offsets(i.id,$2) r where i.id=$1")
            .bind(item).bind(h.super_admin).fetch_one(pool).await.unwrap();
        assert_eq!(
            after, before,
            "upgrade must not rewrite existing identities or versions"
        );
    }
    let mut split_tx = pool.begin().await.unwrap();
    let mut scope = WriteScope::begin(
        &mut split_tx,
        h.id,
        ScopeRequest {
            items: vec![item],
            splits: vec![(item, 1)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let row = scope.item(item).await.unwrap();
    let (_, tail) = scope.split(item, row.version, 1).await.unwrap();
    if concurrent {
        // Split is paused AFTER transferring the projection. A second connection
        // still sees the committed head and must commit production before finish.
        tokio::time::timeout(std::time::Duration::from_secs(3), produce(pool, fired_at))
            .await
            .expect("producer can commit the pre-split identity");
        let count: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='reminder' and occurrence_id=$1")
            .bind(occurrence).fetch_one(pool).await.unwrap();
        assert_eq!(count, 1, "old-identity production committed before split");
    }
    let row = scope.item(tail).await.unwrap();
    let mut fields = ItemWrite::from(&row);
    fields.title = "Renamed series".into();
    scope.update(tail, row.version, &fields).await.unwrap();
    scope
        .finish(if concurrent {
            fired_at
        } else {
            at("2026-10-06T11:29:00Z")
        })
        .await
        .unwrap();
    split_tx.commit().await.unwrap();
    produce(pool, fired_at).await;
    produce(pool, fired_at).await;
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='reminder' and occurrence_id=$1")
        .bind(occurrence).fetch_one(pool).await.unwrap();
    assert_eq!(
        count, 1,
        "split must retain one delivery identity: explicit={explicit}, concurrent={concurrent}"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn split_and_concurrent_reminder_production_dedupe(pool: PgPool) {
    for explicit in [false, true] {
        split_delivery(&pool, explicit, true, false).await;
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn split_after_clock_rewind_does_not_replay(pool: PgPool) {
    for explicit in [false, true] {
        split_delivery(&pool, explicit, false, false).await;
    }
}

async fn legacy_witness(pool: &PgPool, explicit: bool) {
    let legacy = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            porchlight_db::MIGRATOR
                .iter()
                .filter(|m| m.version < 68)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    legacy.run(pool).await.unwrap();
    // The fixture exercises pre-0068 reminder identities using current derivation
    // code, which needs 0073's pure zone expression and 0074's generated column.
    // Both compatibility additions are removed before running the real upgrade.
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../db/migrations/0073_effective_member_zone.sql"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("alter table porchlight.member_agenda add column disposition text")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    split_delivery(pool, explicit, false, true).await;
}

#[sqlx::test(migrations = false)]
async fn legacy_default_witness_survives_identity_upgrade(pool: PgPool) {
    legacy_witness(&pool, false).await;
}

#[sqlx::test(migrations = false)]
async fn legacy_explicit_witness_survives_identity_upgrade(pool: PgPool) {
    legacy_witness(&pool, true).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn internal_reminder_identities_stay_out_of_item_reads(pool: PgPool) {
    use porchlight_api::testing::{TestClient, TestServer};
    use porchlight_core::auth::session::Platform;
    use serde_json::json;

    let s = TestServer::start(&pool).await;
    let device = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&device));
    let created = client
        .request(
            s.lan,
            device.request(
                "POST",
                "/v1/items",
                json!({
                    "title":"Delivery identity", "reminder_offsets":[30]
                }),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(created.status, 201, "{}", created.json());
    let snapshot = client
        .request(
            s.lan,
            device.request("GET", "/v1/snapshot", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(snapshot.status, 200, "{}", snapshot.json());
    let snapshot = snapshot.json();
    let item = &snapshot["items"][0];
    assert_eq!(item["reminders"].as_array().unwrap().len(), 1);
    assert!(
        item.get("reminder_origin_id").is_none(),
        "internal origin must not expand the item wire model"
    );
    assert!(
        item["reminders"][0].get("delivery_id").is_none(),
        "internal delivery identity must not expand the reminder wire model"
    );
}
