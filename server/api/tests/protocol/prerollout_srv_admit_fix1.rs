use axum::{body::to_bytes, extract::State, response::IntoResponse};
use porchlight_api::{
    extract::Caller,
    routes::reads,
    testing::{TestOptions, TestServer},
};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

fn caller(s: &TestServer, member: Uuid, device: Uuid) -> Caller {
    Caller::Member {
        member_id: member,
        user_id: Uuid::nil(),
        role: "super_admin".into(),
        device_id: device,
        session_id: Uuid::nil(),
        household_id: s.init.household_id,
        client_addr: None,
        request_id: Uuid::now_v7(),
    }
}

async fn seed_member(pool: &PgPool, s: &TestServer) -> Uuid {
    sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Sam Rivera','dependent','#123456','UTC') returning id")
        .bind(s.init.household_id).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn six_member_snapshot_burst_waits_instead_of_rejecting(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(s.state.snapshot_slots.available_permits(), 4);
    let mut members = Vec::new();
    for _ in 0..6 {
        members.push(seed_member(&pool, &s).await);
    }
    use futures_util::{StreamExt, stream::FuturesUnordered};
    let mut requests: FuturesUnordered<_> = members
        .into_iter()
        .map(|member| {
            let state = s.state.clone();
            let c = caller(&s, member, Uuid::now_v7());
            async move { reads::snapshot(State(state), c).await.into_response() }
        })
        .collect();
    let mut held = Vec::new();
    for _ in 0..4 {
        let response = requests.next().await.unwrap();
        assert_eq!(
            response.status(),
            200,
            "all six devices must refresh successfully"
        );
        held.push(response);
    }
    assert_eq!(s.state.snapshot_slots.available_permits(), 0);
    // Four unread bodies own the global budget; both remaining requests must wait.
    assert!(futures_util::poll!(requests.next()).is_pending());
    assert_eq!(requests.len(), 2);
    for response in held {
        to_bytes(response.into_body(), usize::MAX).await.unwrap();
    }
    while let Some(response) = requests.next().await {
        assert_eq!(
            response.status(),
            200,
            "queued devices must succeed after bodies drain"
        );
        to_bytes(response.into_body(), usize::MAX).await.unwrap();
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unread_bodies_timeout_global_wait_without_a_database_connection(pool: PgPool) {
    let mut opts = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    opts.settings.snapshot_wait = Duration::from_millis(40);
    let s = TestServer::start_with(&pool, opts).await;
    let mut held = Vec::new();
    for _ in 0..4 {
        let member = seed_member(&pool, &s).await;
        held.push(
            reads::snapshot(State(s.state.clone()), caller(&s, member, Uuid::now_v7()))
                .await
                .unwrap(),
        );
    }
    let device = Uuid::now_v7();
    let c = caller(&s, s.init.admin_member_id, device);
    // Exhaust the request pool: admission must time out without trying to use it.
    let mut connections = Vec::new();
    for _ in 0..s.state.pool.options().get_max_connections() {
        connections.push(s.state.pool.acquire().await.unwrap());
    }
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        reads::snapshot(State(s.state.clone()), c.clone()),
    )
    .await
    .expect("snapshot wait must not acquire a DB connection")
    .into_response();
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()["retry-after"], "2");
    assert_eq!(s.state.pool.num_idle(), 0);
    drop(connections);
    drop(held);
    let response = reads::snapshot(State(s.state.clone()), c).await.unwrap();
    assert_eq!(
        response.status(),
        200,
        "timed out waiter must release its device permit"
    );
    drop(response);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn duplicate_device_snapshot_is_immediate_even_while_queued(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let member = s.init.admin_member_id;
    let mut held = Vec::new();
    for _ in 0..3 {
        held.push(
            reads::snapshot(State(s.state.clone()), caller(&s, member, Uuid::now_v7()))
                .await
                .unwrap(),
        );
    }
    let c = caller(&s, member, Uuid::now_v7());
    let mut waiting = Box::pin(s.state.admit_snapshot(&c));
    assert!(futures_util::poll!(&mut waiting).is_pending());
    let response = tokio::time::timeout(
        Duration::from_millis(100),
        reads::snapshot(State(s.state.clone()), c.clone()),
    )
    .await
    .expect("same device must reject immediately, not wait 15 seconds")
    .into_response();
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()["retry-after"], "1");
    drop(waiting);
    drop(held);
    let response = reads::snapshot(State(s.state.clone()), c).await.unwrap();
    assert_eq!(
        response.status(),
        200,
        "cancellation must release the waiting device permit"
    );
    drop(response);
    s.stop().await;
}

#[tokio::test]
async fn member_snapshot_wait_is_fifo_and_cancel_safe() {
    use porchlight_api::concurrency::MemberSlots;
    let slots = MemberSlots::new(1);
    let member = Uuid::now_v7();
    let held = slots.acquire(member).await.unwrap();
    let mut cancelled = Box::pin(slots.acquire(member));
    let mut first = Box::pin(slots.acquire(member));
    let mut second = Box::pin(slots.acquire(member));
    assert!(futures_util::poll!(&mut cancelled).is_pending());
    assert!(futures_util::poll!(&mut first).is_pending());
    assert!(futures_util::poll!(&mut second).is_pending());
    drop(cancelled);
    drop(held);
    assert!(
        futures_util::poll!(&mut second).is_pending(),
        "later waiter cannot skip the first"
    );
    let first = first.await.unwrap();
    assert!(futures_util::poll!(&mut second).is_pending());
    drop(first);
    drop(second.await.unwrap());
    assert!(slots.acquire(member).await.is_ok());
}

#[test]
fn snapshot_wait_defaults_to_fifteen_seconds_and_is_configurable() {
    use clap::Parser;
    use porchlight_api::config::ServeArgs;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: ServeArgs,
    }
    let parse = |extra: &[&str]| {
        Cli::try_parse_from(
            [
                "test",
                "--database-url",
                "postgres://localhost/test",
                "--enroll-host",
                "localhost",
                "--enroll-cidrs",
                "192.168.77.0/24",
            ]
            .into_iter()
            .chain(extra.iter().copied()),
        )
        .unwrap()
        .args
    };
    assert_eq!(parse(&[]).settings().snapshot_wait, Duration::from_secs(15));
    let configured = parse(&["--snapshot-wait-secs", "2"]);
    configured.validate().unwrap();
    assert_eq!(configured.settings().snapshot_wait, Duration::from_secs(2));
    assert!(parse(&["--snapshot-wait-secs", "0"]).validate().is_err());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn three_downloads_per_device_are_normal_use(pool: PgPool) {
    use porchlight_api::{
        extract::ApiQuery,
        routes::exports::{self, MemberParams},
    };
    let s = TestServer::start_with(&pool, TestOptions::default()).await;
    let c = caller(&s, s.init.admin_member_id, Uuid::now_v7());
    let mut bodies = Vec::new();
    for _ in 0..3 {
        let response = exports::member(
            State(s.state.clone()),
            c.clone(),
            ApiQuery(MemberParams { member_id: None }),
        )
        .await
        .into_response();
        assert_eq!(
            response.status(),
            200,
            "two thumbnails plus one original must fit the device budget"
        );
        bodies.push(response);
    }
    let fourth = exports::member(
        State(s.state.clone()),
        c,
        ApiQuery(MemberParams { member_id: None }),
    )
    .await
    .into_response();
    assert_eq!(fourth.status(), 429);
    drop(bodies);
    s.stop().await;
}
