use chrono::{DateTime, Duration, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    derive, items,
    testing::{Role, connect_as, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn projection_bounds_hidden_cards_and_board_only_footprint(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut cards = Vec::new();
    let mut parents = Vec::new();
    let mut futures = Vec::new();
    for key in ["4080", "6080", "8080", "a080"] {
        let id = insert_appointment(&pool, &h).await.unwrap();
        parents.push(id);
        let card:Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,board_position) values($1,$2,0,$3) returning id").bind(h.id).bind(id).bind(key).fetch_one(&pool).await.unwrap();
        cards.push(card);
        let future:Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,1) returning id").bind(h.id).bind(id).fetch_one(&pool).await.unwrap();
        futures.push(future);
    }
    // Hidden other-member card is still in the projection domain. Future rows are not.
    sqlx::query("update porchlight.items set owner_id=$2,visibility='private' where id=$1")
        .bind(parents[1])
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            board_moves: vec![items::BoardMove {
                occurrence: cards[3],
                before: Some(cards[0]),
                after: Some(cards[2]),
                status: core::Status::Todo,
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    scope
        .move_on_board(cards[3], 1, Some(cards[0]), Some(cards[2]))
        .await
        .unwrap();
    let mut probe = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.items where id=any($1) order by id for update nowait")
        .bind(&parents)
        .fetch_all(&mut *probe)
        .await
        .expect("board must not lock parents");
    sqlx::query(
        "select id from porchlight.item_occurrences where id=any($1) order by id for update nowait",
    )
    .bind(&futures)
    .fetch_all(&mut *probe)
    .await
    .expect("board must not lock future occurrences");
    sqlx::query(
        "select id from porchlight.household_members where household_id=$1 for update nowait",
    )
    .bind(h.id)
    .fetch_all(&mut *probe)
    .await
    .expect("board must not lock members");
    sqlx::query(
        "select id from porchlight.item_occurrences where id=any($1) order by id for update nowait",
    )
    .bind(vec![cards[0], cards[1], cards[2]])
    .fetch_all(&mut *probe)
    .await
    .expect("untied bounds stay unlocked");
    probe.rollback().await.unwrap();
    scope.finish(now()).await.unwrap();
    tx.commit().await.unwrap();
    let order:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.item_occurrences where id=any($1) order by board_position collate \"C\",id").bind(&cards).fetch_all(&pool).await.unwrap();
    assert_eq!(order, vec![cards[0], cards[3], cards[1], cards[2]]);
    let rewritten: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_occurrences where id=any($1) and version<>1",
    )
    .bind(futures)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rewritten, 0);
    let derived: i64 =
        sqlx::query_scalar("select count(*) from porchlight.outbox where kind='derived'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(derived, 0);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn thousand_tied_future_rows_twenty_projection_cards(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut cards = Vec::new();
    for _ in 0..20 {
        let id = insert_appointment(&pool, &h).await.unwrap();
        sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq) select $1,$2,n from generate_series(0,50) n").bind(h.id).bind(id).execute(&pool).await.unwrap();
        let card: Uuid = sqlx::query_scalar(
            "select id from porchlight.item_occurrences where item_id=$1 and seq=0",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        cards.push(card);
    }
    cards.sort();
    let mover = cards.pop().unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::move_on_board(
        &mut tx,
        h.id,
        mover,
        1,
        Some(cards[8]),
        Some(cards[12]),
        now(),
    )
    .await
    .unwrap();
    let mut probe = pool.begin().await.unwrap();
    sqlx::query(
        "select id from porchlight.items where household_id=$1 order by id for update nowait",
    )
    .bind(h.id)
    .fetch_all(&mut *probe)
    .await
    .expect("tied move must not lock parents");
    sqlx::query("select id from porchlight.item_occurrences where household_id=$1 and seq>0 order by id for update nowait").bind(h.id).fetch_all(&mut *probe).await.expect("tied move must not lock future rows");
    sqlx::query("select id from porchlight.item_occurrences where household_id=$1 and seq=0 order by id for key share nowait").bind(h.id).fetch_all(&mut *probe).await.expect("board-only locks must be NKU, not UPDATE");
    probe.rollback().await.unwrap();
    tx.commit().await.unwrap();
    cards.insert(9, mover);
    let order:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.item_occurrences where seq=0 order by board_position collate \"C\",id").fetch_all(&pool).await.unwrap();
    assert_eq!(order, cards);
    let counts:(i64,i64)=sqlx::query_as("select count(*) filter(where version>1),count(*) filter(where seq>0 and version>1) from porchlight.item_occurrences").fetch_one(&pool).await.unwrap();
    assert!(
        counts.0 > 0 && counts.0 <= 20,
        "projection run bound: {counts:?}"
    );
    assert_eq!(counts.1, 0);
    let events: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.outbox where kind='occurrence.changed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let key_changes: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_occurrences where board_position<>'80'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, key_changes);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn split_uuid_order_is_irrelevant_and_emits_reparent_events(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    for id in [
        Uuid::parse_str("ffffffff-ffff-7fff-bfff-ffffffffffff").unwrap(),
        Uuid::new_v4(),
    ] {
        let mut agent = connect_as(&pool, Role::Agent, Some("agent:split-order"))
            .await
            .unwrap();
        sqlx::query("insert into porchlight.items(id,household_id,owner_id,title,kind,time_mode,tzid,due_at,rrule,recurrence_mode) values($1,$2,$3,'head','deadline','fixed','UTC','2026-10-06T12:00Z','FREQ=DAILY;COUNT=4','calendar')").bind(id).bind(h.id).bind(h.user).execute(&mut agent).await.unwrap();
        sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)").bind(h.id).bind(id).execute(&mut agent).await.unwrap();
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        sqlx::query("select id from porchlight.items where id=$1 for update")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        derive::derive_item(&mut tx, id, now()).await.unwrap();
        tx.commit().await.unwrap();
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        let rows = derive::occurrences(&mut tx, id).await.unwrap();
        let version = derive::load_item(&mut tx, id).await.unwrap().version;
        let out = items::split(&mut tx, h.id, id, version, 1, now())
            .await
            .expect("tail must not derive before stale head projections are removed");
        let tail = derive::occurrences(&mut tx, out.tail_id).await.unwrap();
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].id, rows[1].id);
        let counts:(i64,i64)=sqlx::query_as("select (select count(*) from porchlight.member_agenda where item_id=$1),(select count(*) from porchlight.scheduled_reminders where item_id=$1)").bind(out.tail_id).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            counts,
            (3, 3),
            "reparented agenda and reminders rebuilt on tail"
        );
        tx.commit().await.unwrap();
        let events:i64=sqlx::query_scalar("select count(*) from porchlight.outbox where kind='occurrence.changed' and entity_id=any($1)").bind(tail.iter().map(|o|o.id).collect::<Vec<_>>()).fetch_one(&pool).await.unwrap();
        assert_eq!(events, 3);
    }
}

async fn completion(pool: &PgPool) -> (porchlight_db::testing::Household, PgPool, Uuid) {
    let h = seed_household(pool).await.unwrap();
    let app = pool_as(pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "completion".into(),
        timing: core::SeriesInput {
            due_date: Some("2026-10-06".parse().unwrap()),
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
    tx.commit().await.unwrap();
    (h, app, out.item_id)
}
async fn status(
    app: &PgPool,
    h: porchlight_db::testing::Household,
    id: Uuid,
    seq: u32,
    s: core::Status,
    t: DateTime<Utc>,
) {
    let mut tx = begin_as(app, &Actor::system()).await.unwrap();
    let o = derive::occurrences(&mut tx, id)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.anchor.seq == seq)
        .unwrap();
    items::set_status(&mut tx, h.id, o.id, o.version, s, Some(h.user), t)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn latest_closure_not_highest_sequence(pool: PgPool) {
    let (h, app, id) = completion(&pool).await;
    status(&app, h, id, 0, core::Status::Done, now()).await;
    status(
        &app,
        h,
        id,
        1,
        core::Status::Done,
        now() + Duration::days(1),
    )
    .await;
    status(
        &app,
        h,
        id,
        0,
        core::Status::Todo,
        now() + Duration::days(2),
    )
    .await;
    status(
        &app,
        h,
        id,
        0,
        core::Status::Done,
        now() + Duration::days(4),
    )
    .await;
    let mut c = app.acquire().await.unwrap();
    let rows = derive::occurrences(&mut c, id).await.unwrap();
    let opens: Vec<_> = rows.iter().filter(|o| o.status.is_open()).collect();
    assert_eq!(opens.len(), 1);
    assert_eq!(
        opens[0].anchor.due_date,
        Some("2026-10-16".parse().unwrap())
    );
    drop(c);
    // Seq 0 was reclosed later than seq 1: it is still the latest closure for R31.
    status(
        &app,
        h,
        id,
        0,
        core::Status::Todo,
        now() + Duration::days(5),
    )
    .await;
    let mut c = app.acquire().await.unwrap();
    let rows = derive::occurrences(&mut c, id).await.unwrap();
    let opens: Vec<_> = rows.iter().filter(|o| o.status.is_open()).collect();
    assert_eq!(
        opens.len(),
        1,
        "latest-by-time reopen drops untouched seq 2"
    );
    assert_eq!(opens[0].anchor.seq, 0);
    assert_eq!(
        opens[0].anchor.due_date,
        Some("2026-10-06".parse().unwrap())
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn latest_reopen_deletes_only_untouched_successor(pool: PgPool) {
    let (h, app, id) = completion(&pool).await;
    status(&app, h, id, 0, core::Status::Done, now()).await;
    status(
        &app,
        h,
        id,
        0,
        core::Status::Todo,
        now() + Duration::days(1),
    )
    .await;
    let mut c = app.acquire().await.unwrap();
    let rows = derive::occurrences(&mut c, id).await.unwrap();
    assert_eq!(rows.iter().filter(|o| o.status.is_open()).count(), 1);
    assert_eq!(rows[0].anchor.due_date, Some("2026-10-06".parse().unwrap()));
    drop(c);
    status(
        &app,
        h,
        id,
        0,
        core::Status::Done,
        now() + Duration::days(2),
    )
    .await;
    // Move then restore default key: "ever moved" must remain touched.
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let rows = derive::occurrences(&mut tx, id).await.unwrap();
    let next = rows.iter().find(|o| o.status.is_open()).unwrap().clone();
    items::move_on_board(&mut tx, h.id, next.id, next.version, None, None, now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    sqlx::query("update porchlight.item_occurrences set board_position='80' where id=$1")
        .bind(next.id)
        .execute(&pool)
        .await
        .unwrap();
    status(
        &app,
        h,
        id,
        0,
        core::Status::Todo,
        now() + Duration::days(3),
    )
    .await;
    let mut c = app.acquire().await.unwrap();
    let rows = derive::occurrences(&mut c, id).await.unwrap();
    assert_eq!(rows.iter().filter(|o| o.status.is_open()).count(), 2);
    assert!(
        rows.iter()
            .any(|o| o.id == next.id && o.anchor == next.anchor)
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn rebalance_bystander_successor_is_still_untouched(pool: PgPool) {
    let (h, app, id) = completion(&pool).await;
    status(&app, h, id, 0, core::Status::Done, now()).await;
    let mut c = app.acquire().await.unwrap();
    let successor = derive::occurrences(&mut c, id)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.status.is_open())
        .unwrap();
    drop(c);
    let mut others = Vec::new();
    for _ in 0..2 {
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        let out = items::create(
            &mut tx,
            h.id,
            &items::ItemWrite {
                owner_id: h.user,
                title: "bystander".into(),
                ..Default::default()
            },
            &Default::default(),
            now(),
        )
        .await
        .unwrap();
        others.push(derive::occurrences(&mut tx, out.item_id).await.unwrap()[0].clone());
        tx.commit().await.unwrap();
    }
    assert!(successor.id < others[0].id && others[0].id < others[1].id);
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::move_on_board(
        &mut tx,
        h.id,
        others[1].id,
        others[1].version,
        Some(successor.id),
        Some(others[0].id),
        now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut c = app.acquire().await.unwrap();
    let updated = derive::occurrences(&mut c, id)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.id == successor.id)
        .unwrap();
    assert_ne!(updated.board_position, successor.board_position);
    assert!(
        !updated.board_moved,
        "automatic respread is not an explicit move"
    );
    drop(c);
    status(
        &app,
        h,
        id,
        0,
        core::Status::Todo,
        now() + Duration::days(1),
    )
    .await;
    let mut c = app.acquire().await.unwrap();
    let rows = derive::occurrences(&mut c, id).await.unwrap();
    assert_eq!(rows.iter().filter(|o| o.status.is_open()).count(), 1);
    assert!(!rows.iter().any(|o| o.id == successor.id));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn app_and_raw_reskip_record_fresh_times(pool: PgPool) {
    let (h, app, id) = completion(&pool).await;
    status(&app, h, id, 0, core::Status::Skipped, now()).await;
    status(
        &app,
        h,
        id,
        0,
        core::Status::Todo,
        now() + Duration::days(1),
    )
    .await;
    status(
        &app,
        h,
        id,
        0,
        core::Status::Skipped,
        now() + Duration::days(2),
    )
    .await;
    let stamp: DateTime<Utc> = sqlx::query_scalar(
        "select skipped_at from porchlight.item_occurrences where item_id=$1 and seq=0",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stamp, now() + Duration::days(2));
    status(
        &app,
        h,
        id,
        0,
        core::Status::Todo,
        now() + Duration::days(3),
    )
    .await;
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:reskip"))
        .await
        .unwrap();
    sqlx::query(
        "update porchlight.item_occurrences set status='skipped' where item_id=$1 and seq=0",
    )
    .bind(id)
    .execute(&mut agent)
    .await
    .unwrap();
    let fresh: DateTime<Utc> = sqlx::query_scalar(
        "select skipped_at from porchlight.item_occurrences where item_id=$1 and seq=0",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(fresh, stamp);
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    derive::derive_item(&mut tx, id, fresh).await.unwrap();
    let rows = derive::occurrences(&mut tx, id).await.unwrap();
    assert_eq!(rows.iter().filter(|o| o.status.is_open()).count(), 1);
    let expected = core::next_after_completion(
        &derive::load_item(&mut tx, id)
            .await
            .unwrap()
            .spec()
            .unwrap(),
        fresh,
        chrono_tz::America::New_York,
    )
    .unwrap();
    assert_eq!(
        rows.iter()
            .find(|o| o.status.is_open())
            .unwrap()
            .anchor
            .due_date,
        expected.due_date
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn second_scope_is_rejected_in_same_transaction(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let scope = items::WriteScope::begin(&mut tx, h.id, Default::default())
        .await
        .unwrap();
    drop(scope);
    assert!(matches!(
        items::WriteScope::begin(&mut tx, h.id, Default::default()).await,
        Err(derive::Error::ScopeAlreadyUsed)
    ));
    tx.rollback().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::WriteScope::begin(&mut tx, h.id, Default::default())
        .await
        .unwrap()
        .finish(now())
        .await
        .unwrap();
    assert!(matches!(
        items::WriteScope::begin(&mut tx, h.id, Default::default()).await,
        Err(derive::Error::ScopeAlreadyUsed)
    ));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn zone_enqueue_only_locks_deleted_jobs_but_not_projections_or_peers(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set owner_id=$2,deleted_at=now() where id=$1")
        .bind(id)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let peer = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(h.id)
    .bind(peer)
    .bind(h.super_admin)
    .execute(&pool)
    .await
    .unwrap();
    let occurrence:Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0) returning id").bind(h.id).bind(id).fetch_one(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            member_zones: vec![h.user],
            enqueue_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut probe = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.item_occurrences where id=$1 for update nowait")
        .bind(occurrence)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update nowait")
        .bind(peer)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    sqlx::query("select id from porchlight.household_members where id=$1 for update nowait")
        .bind(h.super_admin)
        .fetch_one(&mut *probe)
        .await
        .unwrap();
    probe.rollback().await.unwrap();
    let mut probe = pool.begin().await.unwrap();
    let error = sqlx::query(
        "select item_id from porchlight.item_derivation where item_id=$1 for update nowait",
    )
    .bind(id)
    .fetch_one(&mut *probe)
    .await
    .unwrap_err();
    assert!(matches!(error,sqlx::Error::Database(ref e) if e.code().as_deref()==Some("55P03")));
    probe.rollback().await.unwrap();
    // A new owner/subject/assignee reference cannot sneak in after the final plan.
    let mut probe = pool.begin().await.unwrap();
    sqlx::query("set local lock_timeout='100ms'")
        .execute(&mut *probe)
        .await
        .unwrap();
    let error=sqlx::query("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'late reference','todo')").bind(h.id).bind(h.user).execute(&mut *probe).await.unwrap_err();
    assert!(matches!(error,sqlx::Error::Database(ref e) if e.code().as_deref()==Some("55P03")));
    probe.rollback().await.unwrap();
    drop(scope);
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn all_edit_seq_zero_does_not_lock_closed_history(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set recurrence_mode='calendar',rrule='FREQ=DAILY;COUNT=101' where id=$1").bind(id).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at) select $1,$2,n,'done',now() from generate_series(0,99) n").bind(h.id).bind(id).execute(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let version = derive::load_item(&mut tx, id).await.unwrap().version;
    let out = items::split(&mut tx, h.id, id, version, 0, now())
        .await
        .unwrap();
    assert_eq!(out.head_id, None);
    assert_eq!(out.tail_id, id);
    let mut probe = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.item_occurrences where item_id=$1 and status='done' order by id for update nowait").bind(id).fetch_all(&mut *probe).await.expect("all edit must not lock closed history");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn only_after_and_no_bounds_and_done_window(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let mut cards = Vec::new();
    for (seq, key) in ["4080", "6080", "8080"].iter().enumerate() {
        let card:Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at,board_position) values($1,$2,$3,'done',now(),$4) returning id").bind(h.id).bind(id).bind(seq as i32).bind(key).fetch_one(&pool).await.unwrap();
        cards.push(card);
    }
    let hidden:Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at,board_position) values($1,$2,3,'done',now()-interval '8 days','7080') returning id").bind(h.id).bind(id).fetch_one(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::move_on_board(&mut tx, h.id, cards[0], 1, None, Some(cards[2]), now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let order:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.item_occurrences where id=any($1) order by board_position collate \"C\",id").bind(&cards).fetch_all(&pool).await.unwrap();
    assert_eq!(order, vec![cards[1], cards[0], cards[2]]);
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::move_on_board(&mut tx, h.id, cards[1], 1, None, None, now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let order:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.item_occurrences where id=any($1) order by board_position collate \"C\",id").bind(&cards).fetch_all(&pool).await.unwrap();
    assert_eq!(order, vec![cards[0], cards[2], cards[1]]);
    let unchanged: i32 =
        sqlx::query_scalar("select version from porchlight.item_occurrences where id=$1")
            .bind(hidden)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unchanged, 1);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn terminal_peer_is_not_locked_or_redrained_and_is_excluded(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let peer = insert_appointment(&pool, &h).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(peer)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    derive::derive_item(&mut tx, peer, now()).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query("update porchlight.items set rrule='FREQ=DAILY;COUNT=2;COUNT=3',recurrence_mode='calendar' where id=$1").bind(peer).execute(&pool).await.unwrap();
    sqlx::query(
        "update porchlight.item_derivation set attempts=5,last_error='terminal' where item_id=$1",
    )
    .bind(peer)
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            members: vec![h.super_admin],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut probe = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update nowait")
        .bind(peer)
        .fetch_one(&mut *probe)
        .await
        .expect("terminal peer must not be scope locked");
    probe.rollback().await.unwrap();
    let fields = items::ItemWrite::from(
        &derive::load_item(&mut pool.acquire().await.unwrap(), peer)
            .await
            .unwrap(),
    );
    let mut healthy = fields;
    healthy.timing.rrule = None;
    healthy.timing.recurrence_mode = None;
    scope.create(&healthy, &Default::default()).await.unwrap();
    let outcomes = scope.finish(now()).await.unwrap();
    assert!(outcomes.iter().all(|o| o.derived.conflicts.is_empty()));
    tx.commit().await.unwrap();
    let state: (i32, Option<String>) = sqlx::query_as(
        "select attempts,last_error from porchlight.item_derivation where item_id=$1",
    )
    .bind(peer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, (5, Some("terminal".into())));
}
