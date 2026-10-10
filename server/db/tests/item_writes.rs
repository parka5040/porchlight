use chrono::{DateTime, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    derive, items,
    testing::{Role, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn write_drains_raw_pending_conflict_and_leaves_no_job(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let other = insert_appointment(&pool, &h).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let row = derive::load_item(&mut tx, other).await.unwrap();
    let fields = items::ItemWrite::from(&row);
    let outcome = items::create(
        &mut tx,
        h.id,
        &fields,
        &items::Relationships::default(),
        now(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.derived.conflicts.len(), 1);
    assert_eq!(outcome.derived.conflicts[0].other_item_id, Some(other));
    tx.commit().await.unwrap();
    let pending: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_derivation where applied_gen<requested_gen",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn split_gap_tail_persists_both_specs_and_clears_metadata_on_rule_edit(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "Gap".into(),
        timing: core::SeriesInput {
            kind: core::Kind::Deadline,
            tzid: Some("America/New_York".into()),
            due_at: Some("2026-03-07T07:30:00Z".parse().unwrap()),
            rrule: Some("FREQ=DAILY;COUNT=4".into()),
            recurrence_mode: Some(core::RecurrenceMode::Calendar),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let made = items::create(&mut tx, h.id, &fields, &Default::default(), now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let old = derive::load_item(&mut tx, made.item_id).await.unwrap();
    let split = items::split(&mut tx, h.id, old.id, old.version, 1, now())
        .await
        .unwrap();
    let tail = derive::load_item(&mut tx, split.tail_id).await.unwrap();
    assert_eq!(
        tail.nominal_local_anchor,
        Some("2026-03-08T02:30:00".parse().unwrap())
    );
    assert_eq!(tail.nominal_wall_duration, None);
    assert_eq!(tail.series_id, Some(old.id));
    assert_eq!(tail.split_from_item_id, Some(old.id));
    assert_eq!(tail.split_from_seq, Some(1));
    let head = derive::load_item(&mut tx, old.id).await.unwrap();
    assert_eq!(head.timing.rrule.as_deref(), Some("FREQ=DAILY;COUNT=1"));
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut edit = items::ItemWrite::from(&tail);
    edit.timing.rrule = Some("FREQ=DAILY;COUNT=2".into());
    items::update(&mut tx, h.id, tail.id, tail.version, &edit, now())
        .await
        .unwrap();
    let edited = derive::load_item(&mut tx, tail.id).await.unwrap();
    assert_eq!(edited.nominal_local_anchor, None);
    assert_eq!(edited.nominal_wall_duration, None);
    tx.commit().await.unwrap();
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn concurrent_completion_has_one_successor_and_reopen_removes_it(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "Every hour".into(),
        timing: core::SeriesInput {
            recurrence_mode: Some(core::RecurrenceMode::Completion),
            completion_interval: Some(core::PgInterval {
                months: 0,
                days: 0,
                microseconds: 3600000000,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let made = items::create(&mut tx, h.id, &fields, &Default::default(), now())
        .await
        .unwrap();
    let occ = derive::occurrences(&mut tx, made.item_id).await.unwrap()[0].clone();
    tx.commit().await.unwrap();
    let complete = || async {
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        let result = items::set_status(
            &mut tx,
            h.id,
            occ.id,
            occ.version,
            core::Status::Done,
            Some(h.user),
            now(),
        )
        .await;
        match result {
            Ok(_) => {
                tx.commit().await.unwrap();
                true
            }
            Err(derive::Error::VersionConflict) => {
                tx.rollback().await.unwrap();
                false
            }
            Err(e) => panic!("{e}"),
        }
    };
    let (a, b) = tokio::join!(complete(), complete());
    assert_ne!(a, b);
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let rows = derive::occurrences(&mut tx, made.item_id).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().filter(|o| o.status.is_open()).count(), 1);
    let closed = rows.iter().find(|o| o.id == occ.id).unwrap();
    items::set_status(
        &mut tx,
        h.id,
        occ.id,
        closed.version,
        core::Status::Todo,
        Some(h.user),
        now(),
    )
    .await
    .unwrap();
    let rows = derive::occurrences(&mut tx, made.item_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn board_rebalance_never_derives_peers(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "Board".into(),
        ..Default::default()
    };
    let mut rows = Vec::new();
    for _ in 0..3 {
        let made = items::create(&mut tx, h.id, &fields, &Default::default(), now())
            .await
            .unwrap();
        rows.push(derive::occurrences(&mut tx, made.item_id).await.unwrap()[0].clone());
        tx.commit().await.unwrap();
        tx = begin_as(&app, &Actor::system()).await.unwrap();
    }
    rows.sort_by_key(|o| o.id);
    items::move_on_board(
        &mut tx,
        h.id,
        rows[2].id,
        rows[2].version,
        Some(rows[0].id),
        Some(rows[1].id),
        now(),
    )
    .await
    .unwrap();
    let pending: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_derivation where applied_gen<requested_gen",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        pending, 0,
        "board-only drag leaves current generations unchanged"
    );
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn scope_restarts_when_member_wait_reveals_new_items(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut first = begin_as(&app, &Actor::system()).await.unwrap();
    let made = items::create(
        &mut first,
        h.id,
        &items::ItemWrite {
            owner_id: h.user,
            title: "First".into(),
            ..Default::default()
        },
        &Default::default(),
        now(),
    )
    .await
    .unwrap();
    // Make the new item pending again: only pending peers need scope locks now.
    sqlx::query("update porchlight.items set buffer_before_min=1 where id=$1")
        .bind(made.item_id)
        .execute(&mut *first)
        .await
        .unwrap();
    let second_pool = app.clone();
    let (pid_tx, pid_rx) = tokio::sync::oneshot::channel();
    let second = tokio::spawn(async move {
        let mut tx = begin_as(&second_pool, &Actor::system()).await.unwrap();
        let pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        pid_tx.send(pid).unwrap();
        let scope = items::WriteScope::begin(
            &mut tx,
            h.id,
            items::ScopeRequest {
                members: vec![h.user],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        scope.finish(now()).await.unwrap();
        let ids: Vec<uuid::Uuid> = sqlx::query_scalar(
            "select item_id from porchlight.item_derivation where applied_gen=requested_gen",
        )
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        ids
    });
    let pid = pid_rx.await.unwrap();
    // Wait for the member lock, not a timing guess. At this point the second
    // transaction's item snapshot cannot see the first uncommitted create.
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "select coalesce(wait_event_type='Lock',false) from pg_stat_activity where pid=$1",
            )
            .bind(pid)
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    first.commit().await.unwrap();
    let ids = second.await.unwrap();
    assert!(
        ids.contains(&made.item_id),
        "scope must include items committed during member wait"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn unrelated_poison_does_not_block_affected_member_drain(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("insert into porchlight.items(household_id,owner_id,title,kind,due_date,rrule,recurrence_mode) values($1,$2,'Poison','todo','2026-10-12','FREQ=DAILY;COUNT=2;COUNT=3','calendar')").bind(h.id).bind(h.super_admin).execute(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let out = items::create(
        &mut tx,
        h.id,
        &items::ItemWrite {
            owner_id: h.user,
            title: "Unrelated".into(),
            ..Default::default()
        },
        &Default::default(),
        now(),
    )
    .await
    .unwrap();
    assert!(out.derived.conflicts.is_empty());
    tx.commit().await.unwrap();
    let pending: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_derivation where applied_gen<requested_gen",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 1, "only unrelated poison remains pending");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn appointment_gap_tail_and_repeat_split_keep_exact_head_metadata(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "Exact gap length".into(),
        timing: core::SeriesInput {
            kind: core::Kind::Appointment,
            tzid: Some("America/New_York".into()),
            starts_at: Some("2026-03-07T07:10:00Z".parse().unwrap()),
            ends_at: Some("2026-03-07T07:50:00.123456Z".parse().unwrap()),
            rrule: Some("FREQ=DAILY;COUNT=4".into()),
            recurrence_mode: Some(core::RecurrenceMode::Calendar),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let made = items::create(&mut tx, h.id, &fields, &Default::default(), now())
        .await
        .unwrap();
    let original = derive::load_item(&mut tx, made.item_id).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let first = items::split(&mut tx, h.id, original.id, original.version, 1, now())
        .await
        .unwrap();
    let tail = derive::load_item(&mut tx, first.tail_id).await.unwrap();
    assert_eq!(
        tail.nominal_local_anchor,
        Some("2026-03-08T02:10:00".parse().unwrap())
    );
    assert_eq!(tail.nominal_wall_duration, Some(2_400_123_456));
    let spec = tail.spec().unwrap();
    let expected = core::this_and_future(&spec, 1).unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let second = items::split(&mut tx, h.id, tail.id, tail.version, 1, now())
        .await
        .unwrap();
    let saved_head = derive::load_item(&mut tx, tail.id).await.unwrap();
    let saved_tail = derive::load_item(&mut tx, second.tail_id).await.unwrap();
    assert_eq!(
        saved_head.spec().unwrap().input(),
        expected.0.as_ref().unwrap().input()
    );
    assert_eq!(
        saved_head.spec().unwrap().calendar_cursor(),
        expected.0.as_ref().unwrap().calendar_cursor()
    );
    assert_eq!(saved_head.nominal_wall_duration, Some(2_400_123_456));
    assert_eq!(saved_tail.spec().unwrap().input(), expected.1.input());
    assert_eq!(
        saved_tail.spec().unwrap().calendar_cursor(),
        expected.1.calendar_cursor()
    );
    assert_eq!(saved_tail.series_id, Some(original.id));
    let mut non_timing = items::ItemWrite::from(&saved_head);
    non_timing.title = "Retains cursor".into();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::update(
        &mut tx,
        h.id,
        saved_head.id,
        saved_head.version,
        &non_timing,
        now(),
    )
    .await
    .unwrap();
    let retained = derive::load_item(&mut tx, saved_head.id).await.unwrap();
    assert_eq!(retained.nominal_wall_duration, Some(2_400_123_456));
    non_timing.timing.tzid = Some("UTC".into());
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::update(
        &mut tx,
        h.id,
        retained.id,
        retained.version,
        &non_timing,
        now(),
    )
    .await
    .unwrap();
    let cleared = derive::load_item(&mut tx, retained.id).await.unwrap();
    assert_eq!(cleared.nominal_local_anchor, None);
    assert_eq!(cleared.nominal_wall_duration, None);
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn relationships_override_status_and_soft_delete_are_synchronous(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let other: uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,tzid,starts_at,ends_at) values($1,$2,'Pending new member conflict','appointment','America/New_York','2026-03-07T07:10Z','2026-03-07T07:50Z') returning id").bind(h.id).bind(h.super_admin).fetch_one(&mut *tx).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "Fixed overrides".into(),
        timing: core::SeriesInput {
            kind: core::Kind::Appointment,
            tzid: Some("America/New_York".into()),
            starts_at: Some("2026-03-07T07:10:00Z".parse().unwrap()),
            ends_at: Some("2026-03-07T07:50:00Z".parse().unwrap()),
            ..Default::default()
        },
        ..Default::default()
    };
    let made = items::create(
        &mut tx,
        h.id,
        &fields,
        &Default::default(),
        "2026-03-06T12:00:00Z".parse().unwrap(),
    )
    .await
    .unwrap();
    let row = derive::load_item(&mut tx, made.item_id).await.unwrap();
    let clock = "2026-03-06T12:00:00Z".parse().unwrap();
    let sets = items::Relationships {
        assignees: vec![h.super_admin, h.user],
        viewers: vec![h.dependent],
        reminder_offsets: vec![30],
    };
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let changed = items::replace_relationships(&mut tx, h.id, row.id, row.version, &sets, clock)
        .await
        .unwrap();
    assert!(
        changed
            .derived
            .conflicts
            .iter()
            .any(|c| c.other_item_id == Some(other)),
        "new member's raw pending item must be drained"
    );
    let stored = items::relationships(&mut tx, row.id).await.unwrap();
    assert_eq!(stored.assignees, sets.assignees);
    assert_eq!(stored.viewers, sets.viewers);
    assert_eq!(stored.reminder_offsets, sets.reminder_offsets);
    let o = derive::occurrences(&mut tx, row.id).await.unwrap()[0].clone();
    let mut replacement = o.anchor.clone();
    replacement.local_start = Some("2026-03-08T02:10:00".parse().unwrap());
    replacement.local_end = Some("2026-03-08T02:50:00".parse().unwrap());
    // core validates the submitted fixed instants against the local R10 span.
    replacement.starts_at = Some("2026-03-08T07:00:00Z".parse().unwrap());
    replacement.ends_at = Some("2026-03-08T07:40:00Z".parse().unwrap());
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::override_occurrence(&mut tx, h.id, o.id, o.version, &replacement, clock)
        .await
        .unwrap();
    let agenda: Vec<(DateTime<Utc>, DateTime<Utc>)> =
        sqlx::query_as("select starts_at,ends_at from porchlight.member_agenda where item_id=$1")
            .bind(row.id)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(agenda.len(), 2);
    assert!(
        agenda.iter().all(
            |(s, e)| *s == replacement.starts_at.unwrap() && *e == replacement.ends_at.unwrap()
        )
    );
    // Generated stored anchors are untouched; local authoritative overrides drive projection.
    let detached = derive::occurrences(&mut tx, row.id).await.unwrap()[0].clone();
    assert!(detached.is_detached);
    assert_eq!(detached.anchor.starts_at, o.anchor.starts_at);
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::set_status(
        &mut tx,
        h.id,
        o.id,
        detached.version,
        core::Status::Skipped,
        None,
        clock,
    )
    .await
    .unwrap();
    let reminders: i64 =
        sqlx::query_scalar("select count(*) from porchlight.scheduled_reminders where item_id=$1")
            .bind(row.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(reminders, 0);
    let skipped = derive::occurrences(&mut tx, row.id).await.unwrap()[0].clone();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::set_status(
        &mut tx,
        h.id,
        o.id,
        skipped.version,
        core::Status::Todo,
        None,
        clock,
    )
    .await
    .unwrap();
    let row = derive::load_item(&mut tx, row.id).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::soft_delete(&mut tx, h.id, row.id, row.version, clock)
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar("select (select count(*) from porchlight.member_agenda where item_id=$1)+(select count(*) from porchlight.scheduled_reminders where item_id=$1)+(select count(*) from porchlight.item_derivation where item_id=$1 and applied_gen<requested_gen)").bind(row.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(remaining, 0);
    tx.commit().await.unwrap();
}
