use chrono::{DateTime, Duration, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    derive, items,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn distant_single_anchor_and_edit_preserve_identity(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut fields = items::ItemWrite {
        owner_id: h.user,
        title: "Distant".into(),
        timing: core::SeriesInput {
            kind: core::Kind::Appointment,
            time_mode: core::TimeMode::Fixed,
            tzid: Some("UTC".into()),
            starts_at: Some(now() + Duration::days(1095)),
            ends_at: Some(now() + Duration::days(1095) + Duration::hours(1)),
            ..Default::default()
        },
        ..Default::default()
    };
    let out = items::create(
        &mut tx,
        h.id,
        &fields,
        &items::Relationships {
            reminder_offsets: vec![30],
            ..Default::default()
        },
        now(),
    )
    .await
    .unwrap();
    let rows = derive::occurrences(&mut tx, out.item_id).await.unwrap();
    assert_eq!(rows.len(), 1, "single dated item is not horizon-bounded");
    assert_eq!(rows[0].board_position, "80");
    let counts:(i64,i64)=sqlx::query_as("select (select count(*) from porchlight.member_agenda where item_id=$1),(select count(*) from porchlight.scheduled_reminders where item_id=$1)").bind(out.item_id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(counts, (1, 1));
    fields.timing.starts_at = Some(now() + Duration::days(180));
    fields.timing.ends_at = Some(now() + Duration::days(180) + Duration::hours(1));
    let version = derive::load_item(&mut tx, out.item_id)
        .await
        .unwrap()
        .version;
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::update(&mut tx, h.id, out.item_id, version, &fields, now())
        .await
        .unwrap();
    let before = derive::occurrences(&mut tx, out.item_id).await.unwrap()[0].clone();
    fields.timing.starts_at = Some(now() + Duration::days(1095));
    fields.timing.ends_at = Some(now() + Duration::days(1095) + Duration::hours(1));
    let version = derive::load_item(&mut tx, out.item_id)
        .await
        .unwrap()
        .version;
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::update(&mut tx, h.id, out.item_id, version, &fields, now())
        .await
        .unwrap();
    let after = derive::occurrences(&mut tx, out.item_id).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, before.id);
    assert_eq!(after[0].anchor.starts_at, fields.timing.starts_at);
    let counts:(i64,i64)=sqlx::query_as("select (select count(*) from porchlight.member_agenda where item_id=$1),(select count(*) from porchlight.scheduled_reminders where item_id=$1)").bind(out.item_id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(counts, (1, 1));
    tx.rollback().await.unwrap();
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn completion_distant_first_and_skip_successors_reopen(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "Completion".into(),
        timing: core::SeriesInput {
            due_date: Some("2029-10-05".parse().unwrap()),
            recurrence_mode: Some(core::RecurrenceMode::Completion),
            completion_interval: Some(core::PgInterval {
                months: 0,
                days: 7,
                microseconds: 0,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let out = items::create(&mut tx, h.id, &fields, &Default::default(), now())
        .await
        .unwrap();
    let first = derive::occurrences(&mut tx, out.item_id).await.unwrap();
    assert_eq!(first.len(), 1, "first completion anchor is unbounded");
    // Skip before any completion; successor derives from the time the skip was recorded, not the anchor.
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::set_status(
        &mut tx,
        h.id,
        first[0].id,
        first[0].version,
        core::Status::Skipped,
        None,
        now(),
    )
    .await
    .unwrap();
    let rows = derive::occurrences(&mut tx, out.item_id).await.unwrap();
    let successor = rows
        .iter()
        .find(|o| o.status.is_open())
        .expect("skip must create successor")
        .clone();
    assert_eq!(successor.anchor.seq, 1);
    assert_eq!(
        successor.anchor.due_date,
        Some("2026-10-12".parse().unwrap())
    );
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::set_status(
        &mut tx,
        h.id,
        successor.id,
        successor.version,
        core::Status::Done,
        Some(h.user),
        now() + Duration::days(1),
    )
    .await
    .unwrap();
    let rows = derive::occurrences(&mut tx, out.item_id).await.unwrap();
    let open = rows.iter().find(|o| o.status.is_open()).unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::set_status(
        &mut tx,
        h.id,
        open.id,
        open.version,
        core::Status::Skipped,
        None,
        now() + Duration::days(3),
    )
    .await
    .unwrap();
    let rows = derive::occurrences(&mut tx, out.item_id).await.unwrap();
    let last = rows.iter().find(|o| o.status.is_open()).unwrap().clone();
    assert_eq!(last.anchor.seq, 3);
    assert_eq!(last.anchor.due_date, Some("2026-10-15".parse().unwrap()));
    let skipped = rows.iter().find(|o| o.anchor.seq == 2).unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::set_status(
        &mut tx,
        h.id,
        skipped.id,
        skipped.version,
        core::Status::Todo,
        None,
        now() + Duration::days(4),
    )
    .await
    .unwrap();
    let rows = derive::occurrences(&mut tx, out.item_id).await.unwrap();
    assert_eq!(rows.iter().filter(|o| o.status.is_open()).count(), 1);
    assert!(
        !rows.iter().any(|o| o.id == last.id),
        "R31 removes untouched successor"
    );
    assert_eq!(
        rows.iter().find(|o| o.status.is_open()).unwrap().anchor,
        skipped.anchor
    );
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_sql_skip_records_its_own_time_with_and_without_completion(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "Raw skips".into(),
        timing: core::SeriesInput {
            due_date: Some("2026-10-12".parse().unwrap()),
            recurrence_mode: Some(core::RecurrenceMode::Completion),
            completion_interval: Some(core::PgInterval {
                months: 1,
                days: 2,
                microseconds: 0,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let made = items::create(&mut tx, h.id, &fields, &Default::default(), now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut agent = porchlight_db::testing::connect_as(&pool, Role::Agent, Some("agent:raw-skip"))
        .await
        .unwrap();
    for seq in [0, 2] {
        sqlx::query(
            "update porchlight.item_occurrences set status='skipped' where item_id=$1 and seq=$2",
        )
        .bind(made.item_id)
        .bind(seq)
        .execute(&mut agent)
        .await
        .unwrap();
        let stamp: DateTime<Utc> = sqlx::query_scalar(
            "select skipped_at from porchlight.item_occurrences where item_id=$1 and seq=$2",
        )
        .bind(made.item_id)
        .bind(seq)
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        sqlx::query("select id from porchlight.items where id=$1 for update")
            .bind(made.item_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        derive::derive_item(&mut tx, made.item_id, stamp)
            .await
            .unwrap();
        let rows = derive::occurrences(&mut tx, made.item_id).await.unwrap();
        let open = rows.iter().find(|o| o.status.is_open()).unwrap();
        let spec = derive::load_item(&mut tx, made.item_id)
            .await
            .unwrap()
            .spec()
            .unwrap();
        let expected =
            core::next_after_completion(&spec, stamp, chrono_tz::America::New_York).unwrap();
        assert_eq!(open.anchor.seq, (seq + 1) as u32);
        assert_eq!(open.anchor.due_date, expected.due_date);
        assert_eq!(rows.iter().filter(|o| o.status.is_open()).count(), 1);
        if seq == 0 {
            let (id, version) = (open.id, open.version);
            items::set_status(
                &mut tx,
                h.id,
                id,
                version,
                core::Status::Done,
                Some(h.user),
                stamp - Duration::days(1),
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
    }
}
