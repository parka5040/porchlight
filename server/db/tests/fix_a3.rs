use chrono::{DateTime, Duration, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    derive, items, members,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;

fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn enqueue_only_rejects_reopen_and_split(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    for split in [false, true] {
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        let request = items::ScopeRequest {
            member_zones: vec![h.user],
            enqueue_only: true,
            reopens: if split { vec![] } else { vec![Uuid::nil()] },
            splits: if split {
                vec![(Uuid::nil(), 0)]
            } else {
                vec![]
            },
            ..Default::default()
        };
        assert!(
            matches!(items::WriteScope::begin(&mut tx, h.id, request).await,
            Err(derive::Error::Validation(ref e)) if e.field == "scope")
        );
        tx.rollback().await.unwrap();
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn done_board_window_uses_injected_clock(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let made = items::create(
        &mut tx,
        h.id,
        &items::ItemWrite {
            owner_id: h.user,
            title: "clock card".into(),
            ..Default::default()
        },
        &Default::default(),
        now(),
    )
    .await
    .unwrap();
    let o = derive::occurrences(&mut tx, made.item_id).await.unwrap()[0].clone();
    tx.commit().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::set_status(
        &mut tx,
        h.id,
        o.id,
        o.version,
        core::Status::Done,
        Some(h.user),
        now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // Independent of the machine date: boundary inclusive at exactly seven days,
    // excluded one microsecond later, in planning AND the post-acquisition move.
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let o = derive::occurrences(&mut tx, made.item_id).await.unwrap()[0].clone();
    assert!(matches!(
        items::move_on_board(
            &mut tx,
            h.id,
            o.id,
            o.version,
            None,
            None,
            now() + Duration::days(7) + Duration::microseconds(1)
        )
        .await,
        Err(derive::Error::NotFound)
    ));
    tx.rollback().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::move_on_board(
        &mut tx,
        h.id,
        o.id,
        o.version,
        None,
        None,
        now() + Duration::days(7),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn zone_and_completion_fk_cycle_both_orders_twenty_times(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let p = h.super_admin;
    let c = h.user;
    let q = h.dependent;
    assert!(p < c);
    let device: Uuid = sqlx::query_scalar("insert into porchlight.devices(household_id,member_id,label,platform) values($1,$2,'authority','desktop') returning id")
        .bind(h.id).bind(p).fetch_one(&pool).await.unwrap();
    // Related P->C item, and X owned by Q, assigned to C, but not related to P.
    let mut x = Uuid::nil();
    for owner in [p, q] {
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        let out = items::create(
            &mut tx,
            h.id,
            &items::ItemWrite {
                owner_id: owner,
                title: "cycle".into(),
                ..Default::default()
            },
            &items::Relationships {
                assignees: vec![c],
                ..Default::default()
            },
            now(),
        )
        .await
        .unwrap();
        x = out.item_id;
        tx.commit().await.unwrap();
    }
    for authority in [false, true] {
        for zone_first in [false, true] {
            for _ in 0..20 {
                // Both connections only. Starting order varies; T's member phase is
                // deliberately completed before Z's final scan to force the old cycle.
                let (mut t, mut z) = if zone_first {
                    let z = begin_as(&app, &Actor::system()).await.unwrap();
                    (begin_as(&app, &Actor::system()).await.unwrap(), z)
                } else {
                    let t = begin_as(&app, &Actor::system()).await.unwrap();
                    (t, begin_as(&app, &Actor::system()).await.unwrap())
                };
                let z_pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
                    .fetch_one(&mut *z)
                    .await
                    .unwrap();
                let version = members::get(&mut z, p).await.unwrap().version;
                let o = derive::occurrences(&mut t, x).await.unwrap()[0].clone();
                // Mirror T's normal acquisition footprint in protocol order. Holding
                // these first lets T itself observe Z's wait with no third connection.
                sqlx::query("select id from porchlight.items where id=$1 for no key update")
                    .bind(x)
                    .fetch_one(&mut *t)
                    .await
                    .unwrap();
                sqlx::query("select id from porchlight.item_occurrences where id=$1 for update")
                    .bind(o.id)
                    .fetch_one(&mut *t)
                    .await
                    .unwrap();
                sqlx::query(
                    "select item_id from porchlight.item_derivation where item_id=$1 for update",
                )
                .bind(x)
                .fetch_one(&mut *t)
                .await
                .unwrap();
                sqlx::query(
                    "select id from porchlight.household_members where id=$1 for no key update",
                )
                .bind(c)
                .fetch_one(&mut *t)
                .await
                .unwrap();
                let (ready_tx, mut ready_rx) = tokio::sync::oneshot::channel();
                let (release_tx, release_rx) = tokio::sync::oneshot::channel();
                let zone = tokio::spawn(async move {
                    let result = if authority {
                        members::make_authority(&mut z, h.id, p, version, device, now()).await
                    } else {
                        members::set_time_zone(&mut z, h.id, p, version, chrono_tz::UTC, now())
                            .await
                    };
                    let _ = ready_tx.send(result.is_ok());
                    // Keep P locked until the competing completion has been submitted.
                    let _ = release_rx.await;
                    result.unwrap();
                    z.commit().await.unwrap();
                });
                // In old code Z blocks on C while already holding P UPDATE.
                // In fixed code enqueue-only Z acquires P without ever touching C.
                let mut reached = false;
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
                while tokio::time::Instant::now() < deadline {
                    if ready_rx.try_recv().is_ok() {
                        reached = true;
                        break;
                    }
                    let blocked: bool =
                        sqlx::query_scalar("select cardinality(pg_blocking_pids($1))>0")
                            .bind(z_pid)
                            .fetch_one(&mut *t)
                            .await
                            .unwrap();
                    if blocked {
                        reached = true;
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
                assert!(reached, "Z must either acquire P or reach its old C wait");
                let mut scope = items::WriteScope::begin(
                    &mut t,
                    h.id,
                    items::ScopeRequest {
                        occurrences: vec![o.id],
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
                let completion = async {
                    scope.complete(o.id, o.version, p, now()).await.unwrap();
                    // Do not finish: this test rolls T back to repeat the same steady state.
                };
                let release = async {
                    // Let completion attempt its FK KEY SHARE before releasing Z.
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    release_tx.send(()).unwrap();
                    zone.await.unwrap();
                };
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    tokio::join!(completion, release);
                })
                .await
                .unwrap();
                drop(scope);
                t.rollback().await.unwrap();
                // Z commits to test the actual protocol.
                // Ack its pending item outside the pair so no peers enter the next round.
                sqlx::query("update porchlight.item_derivation set applied_gen=requested_gen")
                    .execute(&pool)
                    .await
                    .unwrap();
                // Also exercise actual free primitives through finish+commit, in both
                // operation-submission orders, without the mirrored acquisition gate.
                let mut conn = app.acquire().await.unwrap();
                let version = members::get(&mut conn, p).await.unwrap().version;
                drop(conn);
                let zone_write = async {
                    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
                    if authority {
                        members::make_authority(&mut tx, h.id, p, version, device, now())
                            .await
                            .unwrap();
                    } else {
                        members::set_time_zone(&mut tx, h.id, p, version, chrono_tz::UTC, now())
                            .await
                            .unwrap();
                    }
                    tx.commit().await.unwrap();
                };
                let ordinary_write = async {
                    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
                    items::set_status(
                        &mut tx,
                        h.id,
                        o.id,
                        o.version,
                        core::Status::Done,
                        Some(p),
                        now(),
                    )
                    .await
                    .unwrap();
                    tx.commit().await.unwrap();
                };
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    if zone_first {
                        tokio::join!(zone_write, ordinary_write);
                    } else {
                        tokio::join!(ordinary_write, zone_write);
                    }
                })
                .await
                .unwrap();
                // Restore the authoritative state via a separate ordinary scope so
                // the next pair starts converged, not via a test-only version reset.
                let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
                let closed = derive::occurrences(&mut tx, x).await.unwrap()[0].clone();
                assert_eq!(closed.status, core::Status::Done);
                assert_eq!(closed.completed_by, Some(p));
                items::set_status(
                    &mut tx,
                    h.id,
                    closed.id,
                    closed.version,
                    core::Status::Todo,
                    None,
                    now(),
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
            }
        }
    }
}
