use chrono::{DateTime, Duration, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    derive, items,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-03-06T12:00:00Z".parse().unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn dispositions_each_policy_advance_with_clock_and_dst_series(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    for (policy, expected) in [
        (
            core::MissedPolicy::CarryOver,
            vec!["expired", "expired", "overdue"],
        ),
        (
            core::MissedPolicy::Stack,
            vec!["overdue", "overdue", "overdue"],
        ),
        (
            core::MissedPolicy::Skip,
            vec!["expired", "expired", "expired"],
        ),
    ] {
        let fields = items::ItemWrite {
            title: "DST".into(),
            owner_id: h.user,
            timing: core::SeriesInput {
                kind: core::Kind::Appointment,
                tzid: Some("America/New_York".into()),
                starts_at: Some("2026-03-07T07:10:00Z".parse().unwrap()),
                ends_at: Some("2026-03-07T07:50:00Z".parse().unwrap()),
                rrule: Some("FREQ=DAILY;COUNT=3".into()),
                recurrence_mode: Some(core::RecurrenceMode::Calendar),
                missed_policy: policy,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        let out = items::create(&mut tx, h.id, &fields, &Default::default(), now())
            .await
            .unwrap();
        let id = out.item_id;
        let gap: (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
            "select starts_at,ends_at from porchlight.item_occurrences where item_id=$1 and seq=1",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(
            gap.0,
            "2026-03-08T07:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(gap.1 - gap.0, Duration::minutes(40));
        derive::derive_item(&mut tx, id, "2026-03-10T00:00:00Z".parse().unwrap())
            .await
            .unwrap();
        let states: Vec<String> = sqlx::query_scalar(
            "select disposition from porchlight.item_occurrences where item_id=$1 order by seq",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        assert_eq!(states, expected);
        tx.commit().await.unwrap();
    }
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn overlapping_member_writes_twenty_times_in_both_orders_no_deadlock(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut ids = Vec::new();
    for owner in [h.user, h.super_admin] {
        let fields = items::ItemWrite {
            title: "Shared".into(),
            owner_id: owner,
            ..Default::default()
        };
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        let out = items::create(
            &mut tx,
            h.id,
            &fields,
            &items::Relationships {
                assignees: vec![h.user, h.super_admin],
                ..Default::default()
            },
            now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        ids.push(out.item_id);
    }
    for reverse in [false, true] {
        for n in 0..20 {
            let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
            let edit = |id: Uuid, barrier: std::sync::Arc<tokio::sync::Barrier>| {
                let app = app.clone();
                async move {
                    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
                    let row = derive::load_item(&mut tx, id).await.unwrap();
                    let mut fields = items::ItemWrite::from(&row);
                    fields.title = format!("Edit {n}");
                    barrier.wait().await;
                    items::update(&mut tx, h.id, id, row.version, &fields, now())
                        .await
                        .unwrap();
                    tx.commit().await.unwrap();
                }
            };
            let (a, b) = if reverse {
                (ids[1], ids[0])
            } else {
                (ids[0], ids[1])
            };
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                tokio::join!(edit(a, barrier.clone()), edit(b, barrier));
            })
            .await
            .expect("domain locks must not deadlock");
        }
    }
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn completion_all_edit_regenerates_open_anchor(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut fields = items::ItemWrite {
        title: "Complete".into(),
        owner_id: h.user,
        timing: core::SeriesInput {
            kind: core::Kind::Deadline,
            tzid: Some("America/New_York".into()),
            due_at: Some("2026-03-07T07:00:00Z".parse().unwrap()),
            recurrence_mode: Some(core::RecurrenceMode::Completion),
            completion_interval: Some(core::PgInterval {
                months: 0,
                days: 1,
                microseconds: 0,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let out = items::create(&mut tx, h.id, &fields, &Default::default(), now())
        .await
        .unwrap();
    let row = derive::load_item(&mut tx, out.item_id).await.unwrap();
    fields.timing.due_at = Some("2026-03-07T08:00:00Z".parse().unwrap());
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::update(&mut tx, h.id, row.id, row.version, &fields, now())
        .await
        .unwrap();
    let due: DateTime<Utc> =
        sqlx::query_scalar("select due_at from porchlight.item_occurrences where item_id=$1")
            .bind(row.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(due, fields.timing.due_at.unwrap());
    tx.commit().await.unwrap();
}
