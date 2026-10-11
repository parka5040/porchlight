use chrono::{DateTime, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    derive, items,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use std::sync::Arc;

fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn batched_item_decode_preserves_mixed_timing_variants(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let interval = core::PgInterval {
        months: 1,
        days: 2,
        microseconds: 3_600_000_001,
    };
    let mut ids = Vec::new();
    for timing in [
        core::SeriesInput::default(),
        core::SeriesInput {
            kind: core::Kind::Appointment,
            tzid: Some("America/New_York".into()),
            starts_at: Some(now()),
            ends_at: Some(now() + chrono::Duration::hours(1)),
            ..Default::default()
        },
        core::SeriesInput {
            kind: core::Kind::Deadline,
            time_mode: core::TimeMode::Floating,
            local_due: Some("2026-10-06T10:00:00".parse().unwrap()),
            rrule: Some("FREQ=DAILY;COUNT=2".into()),
            recurrence_mode: Some(core::RecurrenceMode::Calendar),
            ..Default::default()
        },
        core::SeriesInput {
            recurrence_mode: Some(core::RecurrenceMode::Completion),
            completion_interval: Some(interval),
            ..Default::default()
        },
    ] {
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        let made = items::create(
            &mut tx,
            h.id,
            &items::ItemWrite {
                title: "Mixed timing".into(),
                owner_id: h.user,
                timing,
                ..Default::default()
            },
            &Default::default(),
            now(),
        )
        .await
        .unwrap();
        ids.push(made.item_id);
        tx.commit().await.unwrap();
    }
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut requested = ids.clone();
    requested.extend([ids[0], uuid::Uuid::now_v7()]);
    let rows = derive::load_items(&mut tx, &requested).await.unwrap();
    assert_eq!(
        rows.len(),
        ids.len(),
        "missing and duplicate ids add no rows"
    );
    for id in ids {
        let single = derive::load_item(&mut tx, id).await.unwrap();
        let batch = rows.iter().find(|row| row.id == id).unwrap();
        assert_eq!(
            serde_json::to_value(batch).unwrap(),
            serde_json::to_value(single).unwrap()
        );
    }
    assert_eq!(
        rows.iter()
            .filter_map(|row| row.timing.completion_interval)
            .collect::<Vec<_>>(),
        vec![interval]
    );
    assert!(derive::load_items(&mut tx, &[]).await.unwrap().is_empty());
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn concurrent_derivation_creates_only_one_completion_successor(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let made = items::create(
        &mut tx,
        h.id,
        &items::ItemWrite {
            title: "Completion successor".into(),
            owner_id: h.user,
            timing: core::SeriesInput {
                recurrence_mode: Some(core::RecurrenceMode::Completion),
                completion_interval: Some(core::PgInterval {
                    months: 0,
                    days: 0,
                    microseconds: 3_600_000_000,
                }),
                ..Default::default()
            },
            ..Default::default()
        },
        &Default::default(),
        now(),
    )
    .await
    .unwrap();
    let first = derive::occurrences(&mut tx, made.item_id).await.unwrap()[0].id;
    tx.commit().await.unwrap();
    // Raw authoritative completion enqueues derivation without generating the successor.
    sqlx::query("UPDATE porchlight.item_occurrences SET status='done',completed_at=$2,completed_by=$3 WHERE id=$1")
        .bind(first).bind(now()).bind(h.user).execute(&pool).await.unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let derive_once = || {
        let (app, barrier) = (app.clone(), barrier.clone());
        tokio::spawn(async move {
            let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
            barrier.wait().await;
            // No expected-version argument: both contenders must actually derive.
            derive::derive_item(&mut tx, made.item_id, now())
                .await
                .unwrap();
            tx.commit().await.unwrap();
        })
    };
    let (a, b) = tokio::join!(derive_once(), derive_once());
    a.unwrap();
    b.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let rows = derive::occurrences(&mut tx, made.item_id).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().filter(|row| row.status.is_open()).count(), 1);
    assert_eq!(
        rows.iter()
            .filter(|row| row.id == first && row.status == core::Status::Done)
            .count(),
        1
    );
    let successor = rows.iter().find(|row| row.status.is_open()).unwrap().id;
    derive::derive_item(&mut tx, made.item_id, now())
        .await
        .unwrap();
    let again = derive::occurrences(&mut tx, made.item_id).await.unwrap();
    assert_eq!(again.len(), 2);
    assert_eq!(
        again.iter().find(|row| row.status.is_open()).unwrap().id,
        successor
    );
    tx.rollback().await.unwrap();
}
