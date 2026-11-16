use axum::{
    body::to_bytes,
    extract::{Path, RawQuery, State},
};
use porchlight_api::{
    extract::Caller,
    routes::item_sync,
    testing::{TestOptions, TestServer},
};
use serde_json::Value;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn oversized_canonical_sections_return_documented_problem(pool: sqlx::PgPool) {
    use axum::{http::StatusCode, response::IntoResponse};
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let h = s.init.household_id;
    let m = s.init.admin_member_id;
    let id: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,notes,kind) values($1,$2,'Large canonical sections',repeat('界',20000),'todo') returning id")
        .bind(h).bind(m).fetch_one(&pool).await.unwrap();
    // Recent Done cards are canonical board children, not agenda/comment pages.
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at,completed_by) select $1,$2,n,'done',now(),$3 from generate_series(1,150) n")
        .bind(h).bind(id).bind(m).execute(&pool).await.unwrap();
    let caller = Caller::Agent {
        household_id: h,
        token_id: Uuid::now_v7(),
        member_id: m,
        label: "Fixture".into(),
        scopes: vec![],
        request_id: Uuid::now_v7(),
    };
    let error = item_sync::get(State(s.state.clone()), caller, Path(id), RawQuery(None))
        .await
        .unwrap_err();
    assert_eq!(
        error.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "canonical sections exceeding the budget must not return 500"
    );
    let response = error.into_response();
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let problem: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        problem["type"],
        "urn:porchlight:problem:item-sync-too-large"
    );
    assert!(problem.get("coverage").is_none());
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn bundle_deduplicates_and_bounds_large_agenda(pool: sqlx::PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let h = s.init.household_id;
    let m = s.init.admin_member_id;
    let id:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,kind,title,notes,time_mode,tzid,starts_at,ends_at) values($1,$2,'appointment','Large agenda',repeat('界',20000),'fixed','UTC','2026-10-01T09:00Z','2026-10-01T10:00Z') returning id").bind(h).bind(m).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at) select $1,$2,n,'2026-10-01T09:00Z'::timestamptz+n*interval '1 day','2026-10-01T10:00Z'::timestamptz+n*interval '1 day' from generate_series(0,199) n").bind(h).bind(id).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,starts_at,ends_at,busy) select $1,$2,item_id,id,starts_at,ends_at,tstzrange(starts_at,ends_at,'[)') from porchlight.item_occurrences where item_id=$3").bind(h).bind(m).bind(id).execute(&pool).await.unwrap();
    let c = Caller::Agent {
        household_id: h,
        token_id: Uuid::now_v7(),
        member_id: m,
        label: "Fixture".into(),
        scopes: vec![],
        request_id: Uuid::now_v7(),
    };
    let w = format!("{m}:2026-10-01T00:00:00Z:2028-09-30T00:00:00Z");
    let query = std::iter::repeat_n(format!("agenda={w}"), 8)
        .collect::<Vec<_>>()
        .join("&");
    let response = item_sync::get(State(s.state.clone()), c, Path(id), RawQuery(Some(query)))
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        bytes.len() <= 8 * 1024 * 1024,
        "bundle exceeded 8 MiB: {}",
        bytes.len()
    );
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        v["agenda"].as_array().unwrap().len(),
        1,
        "identical windows must be deduplicated"
    );
    assert!(v["agenda"][0]["items"].as_array().unwrap().len() <= 200);
    assert_eq!(v["coverage"]["agenda"][0]["complete"], false);
    s.stop().await;
}
