use axum::{extract::State, response::IntoResponse};
use porchlight_api::{
    extract::{ApiQuery, Caller},
    routes::exports::{self, MemberParams},
    testing::{TestOptions, TestServer},
};
use sqlx::PgPool;
use uuid::Uuid;

fn caller(s: &TestServer) -> Caller {
    Caller::Member {
        member_id: s.init.admin_member_id,
        user_id: Uuid::nil(),
        role: "super_admin".into(),
        device_id: Uuid::nil(),
        session_id: Uuid::nil(),
        household_id: s.init.household_id,
        client_addr: None,
        request_id: Uuid::now_v7(),
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn audit_limits_export_permit_released_after_spooling(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let first = exports::member(
        State(s.state.clone()),
        caller(&s),
        ApiQuery(MemberParams { member_id: None }),
    )
    .await
    .unwrap()
    .into_response();
    let second = exports::member(
        State(s.state.clone()),
        caller(&s),
        ApiQuery(MemberParams { member_id: None }),
    )
    .await
    .into_response();
    assert_eq!(
        second.status(),
        200,
        "an unconsumed completed spool must not hold the household permit"
    );
    drop(first);
    let next = exports::member(
        State(s.state.clone()),
        caller(&s),
        ApiQuery(MemberParams { member_id: None }),
    )
    .await
    .into_response();
    assert_eq!(next.status(), 200);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn audit_limits_large_notes_export_is_complete_in_bounded_chunks(pool: PgPool) {
    use http_body_util::BodyExt;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let notes = "escaped \"\\\n café ".repeat(1000);
    let id: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,kind,title,notes,time_mode,starts_at,ends_at,tzid,recurrence_mode,rrule) values($1,$2,'appointment','Series',$3,'fixed','2026-01-01T09:00Z','2026-01-01T10:00Z','UTC','calendar','FREQ=DAILY;COUNT=1000') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).bind(&notes).fetch_one(&pool).await.unwrap();
    let mut tx = porchlight_db::tx::begin(&pool).await.unwrap();
    porchlight_db::derive::derive_item(&mut tx, id, "2028-01-01T00:00:00Z".parse().unwrap())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let response = exports::member(
        State(s.state.clone()),
        caller(&s),
        ApiQuery(MemberParams { member_id: None }),
    )
    .await
    .unwrap();
    let expected = response.headers()["content-length"]
        .to_str()
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        if let Ok(chunk) = frame.unwrap().into_data() {
            assert!(
                chunk.len() <= 64 * 1024,
                "export body buffers must stay bounded"
            );
            bytes.extend_from_slice(&chunk);
        }
    }
    assert_eq!(bytes.len(), expected);
    assert!(bytes.len() > 20_000_000);
    let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(data["tables"]["items"][0]["notes"], notes);
    assert_eq!(
        data["tables"]["item_occurrences"].as_array().unwrap().len(),
        1000
    );
    let ics = data["appointments_ics"].as_str().unwrap();
    assert_eq!(ics.matches("BEGIN:VEVENT").count(), 1000);
    assert!(ics.ends_with("END:VCALENDAR\r\n"));
    assert_eq!(ics.matches("DESCRIPTION:").count(), 1000);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn audit_limits_failed_spool_releases_household_permit(pool: PgPool) {
    let mut options = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    options.settings.disk_reserve_bytes = u64::MAX;
    let s = TestServer::start_with(&pool, options).await;
    for _ in 0..2 {
        let response = exports::member(
            State(s.state.clone()),
            caller(&s),
            ApiQuery(MemberParams { member_id: None }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), 507);
    }
}
