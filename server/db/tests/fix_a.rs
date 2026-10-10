use chrono::{DateTime, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    derive, items, members,
    testing::{Role, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn related_poison_isolated_and_stale_agenda_excluded(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let peer = insert_appointment(&pool, &h).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let fields = items::ItemWrite::from(&derive::load_item(&mut tx, peer).await.unwrap());
    derive::derive_item(&mut tx, peer, now()).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query("update porchlight.items set rrule='FREQ=DAILY;COUNT=2;COUNT=3',recurrence_mode='calendar' where id=$1").bind(peer).execute(&pool).await.unwrap();
    sqlx::query("update porchlight.item_derivation set attempts=4,last_error='sqlstate:XX000' where item_id=$1").bind(peer).execute(&pool).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let made = items::create(&mut tx, h.id, &fields, &Default::default(), now())
        .await
        .expect("related poison must not fail user write");
    assert!(
        made.derived.conflicts.is_empty(),
        "poisoned stale agenda must be excluded"
    );
    tx.commit().await.unwrap();
    let state: (bool, Option<String>, i32) = sqlx::query_as("select applied_gen<requested_gen,last_error,attempts from porchlight.item_derivation where item_id=$1").bind(peer).fetch_one(&pool).await.unwrap();
    assert_eq!(state, (true, Some("sqlstate:XX000".into()), 4));
    // A still-poisoned raw removal must be found through stale agenda, not current relationships.
    sqlx::query("update porchlight.items set owner_id=$2 where id=$1")
        .bind(peer)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let made_row = derive::load_item(&mut tx, made.item_id).await.unwrap();
    let updated = items::update(
        &mut tx,
        h.id,
        made.item_id,
        made_row.version,
        &fields,
        now(),
    )
    .await
    .unwrap();
    assert!(
        updated.derived.conflicts.is_empty(),
        "removed-member poison must not leak stale conflicts"
    );
    tx.commit().await.unwrap();
    sqlx::query(
        "update porchlight.items set rrule=null,recurrence_mode=null,owner_id=$2 where id=$1",
    )
    .bind(peer)
    .bind(h.user)
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let made_row = derive::load_item(&mut tx, made.item_id).await.unwrap();
    let updated = items::update(
        &mut tx,
        h.id,
        made.item_id,
        made_row.version,
        &fields,
        now(),
    )
    .await
    .unwrap();
    assert!(
        updated.derived.conflicts.is_empty(),
        "removed member's stale agenda must be drained"
    );
    tx.commit().await.unwrap();
    let stale: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.member_agenda where item_id=$1 and member_id=$2",
    )
    .bind(peer)
    .bind(h.super_admin)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stale, 0);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn split_reparents_closed_detached_and_board_state(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "series".into(),
        timing: core::SeriesInput {
            kind: core::Kind::Deadline,
            tzid: Some("UTC".into()),
            due_at: Some("2026-10-06T12:00:00Z".parse().unwrap()),
            rrule: Some("FREQ=DAILY;COUNT=5".into()),
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
    sqlx::query("update porchlight.item_occurrences set status=case seq when 1 then 'done' when 2 then 'skipped' when 3 then 'doing' else status end,completed_at=case when seq=1 then $2 end,completed_by=case when seq=1 then $3 end,board_position='8180',override_local_due=case when seq=4 then timestamp '2026-10-20 12:00' end where item_id=$1").bind(made.item_id).bind(now()).bind(h.user).execute(&pool).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let old = derive::occurrences(&mut tx, made.item_id).await.unwrap();
    let head = derive::load_item(&mut tx, made.item_id).await.unwrap();
    let split = items::split(&mut tx, h.id, head.id, head.version, 1, now())
        .await
        .unwrap();
    let kept = derive::occurrences(&mut tx, head.id).await.unwrap();
    let tail = derive::occurrences(&mut tx, split.tail_id).await.unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(tail.len(), 4);
    for (before, after) in old[1..].iter().zip(&tail) {
        assert_eq!(before.id, after.id);
        assert_eq!(before.status, after.status);
        assert_eq!(before.board_position, after.board_position);
        assert_eq!(before.completed_at, after.completed_at);
        assert_eq!(before.override_local_due, after.override_local_due);
        assert_eq!(before.anchor.due_at, after.anchor.due_at);
        assert_eq!(after.anchor.seq, before.anchor.seq - 1);
    }
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn no_change_report_does_not_wait_for_device_and_revoked_authority_replaced(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut devices = Vec::new();
    for label in ["one", "two"] {
        let id:Uuid=sqlx::query_scalar("insert into porchlight.devices(household_id,member_id,label,platform) values($1,$2,$3,'desktop') returning id").bind(h.id).bind(h.user).bind(label).fetch_one(&pool).await.unwrap();
        devices.push(id);
    }
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let actor = Actor::system();
    assert!(
        members::report_device_zone(&app, &actor, h.user, devices[0], chrono_tz::UTC)
            .await
            .unwrap()
    );
    let mut held = begin_as(&app, &actor).await.unwrap();
    sqlx::query("select id from porchlight.devices where id=$1 for no key update")
        .bind(devices[0])
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        members::report_device_zone(&app, &actor, h.user, devices[0], chrono_tz::UTC),
    )
    .await
    .expect("no-op report waited for mutation's device lock")
    .unwrap();
    assert!(!result);
    held.rollback().await.unwrap();
    sqlx::query("update porchlight.devices set revoked_at=now() where id=$1")
        .bind(devices[0])
        .execute(&pool)
        .await
        .unwrap();
    let authority: Option<Uuid> = sqlx::query_scalar(
        "select tz_authority_device_id from porchlight.household_members where id=$1",
    )
    .bind(h.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        authority, None,
        "revocation must clear the dead authority immediately"
    );
    assert!(
        members::report_device_zone(&app, &actor, h.user, devices[1], chrono_tz::Europe::Paris)
            .await
            .unwrap()
    );
    let mut c = app.acquire().await.unwrap();
    assert_eq!(
        members::get(&mut c, h.user)
            .await
            .unwrap()
            .tz_authority_device_id,
        Some(devices[1])
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn patch_and_split_edits_derive_once_per_target(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let old = derive::load_item(&mut tx, id).await.unwrap();
    let mut fields = items::ItemWrite::from(&old);
    fields.title = "PATCH".into();
    let sets = items::Relationships {
        assignees: vec![h.user],
        ..Default::default()
    };
    let mut scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            items: vec![id],
            members: items::agenda_members(&fields, &sets),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    scope.update(id, old.version, &fields).await.unwrap();
    let version = scope.item(id).await.unwrap().version;
    scope
        .replace_relationships(id, version, &sets)
        .await
        .unwrap();
    assert_eq!(scope.finish(now()).await.unwrap().len(), 1);
    tx.commit().await.unwrap();
    let derived: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.outbox where kind='derived' and entity_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(derived, 1);
    let assigned: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.member_agenda where item_id=$1 and member_id=$2",
    )
    .bind(id)
    .bind(h.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(assigned, 1);
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut fields = items::ItemWrite::from(&derive::load_item(&mut tx, id).await.unwrap());
    fields.timing.rrule = Some("FREQ=DAILY;COUNT=4".into());
    fields.timing.recurrence_mode = Some(core::RecurrenceMode::Calendar);
    let version = derive::load_item(&mut tx, id).await.unwrap().version;
    items::update(&mut tx, h.id, id, version, &fields, now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            items: vec![id],
            splits: vec![(id, 2)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let version = scope.item(id).await.unwrap().version;
    let (head, tail) = scope.split(id, version, 2).await.unwrap();
    assert_eq!(head, Some(id));
    let row = scope.item(tail).await.unwrap();
    let mut fields = items::ItemWrite::from(&row);
    fields.title = "tail edited".into();
    scope.update(tail, row.version, &fields).await.unwrap();
    assert_eq!(scope.finish(now()).await.unwrap().len(), 2);
    assert_eq!(
        derive::load_item(&mut tx, tail).await.unwrap().title,
        "tail edited"
    );
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn scope_footprint_allows_disjoint_writes_fk_inserts_and_closed_history(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let target = insert_appointment(&pool, &h).await.unwrap();
    let other = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set owner_id=$2 where id=$1")
        .bind(other)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, target, now()).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query(
        "update porchlight.item_occurrences set status='done',completed_at=now() where item_id=$1",
    )
    .bind(target)
    .execute(&pool)
    .await
    .unwrap();
    let closed: Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
            .bind(target)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            items: vec![target],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let disjoint = async {
        let mut c = pool.begin().await.unwrap();
        sqlx::query("select id from porchlight.items where id=$1 for update nowait")
            .bind(other)
            .fetch_one(&mut *c)
            .await
            .unwrap();
        sqlx::query("select id from porchlight.item_occurrences where id=$1 for update nowait")
            .bind(closed)
            .fetch_one(&mut *c)
            .await
            .unwrap();
        sqlx::query(
            "select id from porchlight.household_members where id=$1 for no key update nowait",
        )
        .bind(h.user)
        .fetch_one(&mut *c)
        .await
        .unwrap();
        sqlx::query("select id from porchlight.items where id=$1 for key share nowait")
            .bind(target)
            .fetch_one(&mut *c)
            .await
            .unwrap();
        c.rollback().await.unwrap();
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), disjoint)
        .await
        .unwrap();
    let row = scope.item(target).await.unwrap();
    let mut fields = items::ItemWrite::from(&row);
    fields.title = "closed preserved".into();
    scope.update(target, row.version, &fields).await.unwrap();
    scope.finish(now()).await.unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn declared_reopen_prelocks_possible_deletion_before_members(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let target = insert_appointment(&pool, &h).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, target, now()).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query(
        "update porchlight.item_occurrences set status='done',completed_at=now() where item_id=$1",
    )
    .bind(target)
    .execute(&pool)
    .await
    .unwrap();
    let closed: Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
            .bind(target)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut fk = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.item_occurrences where id=$1 for key share")
        .bind(closed)
        .fetch_one(&mut *fk)
        .await
        .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        items::WriteScope::begin(
            &mut tx,
            h.id,
            items::ScopeRequest {
                occurrences: vec![closed],
                reopens: vec![closed],
                ..Default::default()
            },
        ),
    )
    .await;
    assert!(
        result.is_err(),
        "possible deletion must acquire UPDATE, not upgrade NKU under member locks"
    );
    drop(result);
    fk.rollback().await.unwrap();
    tx.rollback().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            reopens: vec![closed],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let version = scope.occurrence(closed).await.unwrap().version;
    scope.reopen(closed, version).await.unwrap();
    assert_eq!(scope.finish(now()).await.unwrap().len(), 1);
    assert_eq!(
        derive::occurrences(&mut tx, target).await.unwrap()[0].status,
        core::Status::Todo
    );
    tx.commit().await.unwrap();
}
