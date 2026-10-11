use axum::{extract::State, http::HeaderMap, response::IntoResponse};
use http_body_util::BodyExt;
use porchlight_api::{
    extract::{ApiQuery, Caller},
    routes::exports::{self, MemberParams},
    testing::{TestOptions, TestServer},
};
use sqlx::PgPool;
use uuid::Uuid;

fn member(s: &TestServer) -> Caller {
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
fn admin(s: &TestServer) -> Caller {
    Caller::Agent {
        token_id: Uuid::nil(),
        member_id: s.init.admin_member_id,
        label: "Export fixture".into(),
        scopes: vec!["admin".into()],
        household_id: s.init.household_id,
        request_id: Uuid::now_v7(),
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn retained_member_spools_are_bounded_without_blocking_admin(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    sqlx::query("INSERT INTO porchlight.items(household_id,owner_id,kind,title,notes) SELECT $1,$2,'todo','Export fixture',repeat('x',20000) FROM generate_series(1,64)")
        .bind(s.init.household_id).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    let mut bodies = Vec::new();
    for _ in 0..2 {
        let response = exports::member(
            State(s.state.clone()),
            member(&s),
            ApiQuery(MemberParams { member_id: None }),
        )
        .await
        .unwrap();
        let mut body = response.into_body();
        assert!(body.frame().await.unwrap().is_ok());
        bodies.push(body);
    }
    assert!(
        s.state.disk_budget.reserved_bytes() > 1024 * 1024,
        "retained file bytes stay charged"
    );
    let third = exports::member(
        State(s.state.clone()),
        member(&s),
        ApiQuery(MemberParams { member_id: None }),
    )
    .await
    .into_response();
    assert_eq!(
        third.status(),
        429,
        "retained spools need an aggregate admission cap"
    );
    assert_eq!(third.headers()["retry-after"], "1");
    let response = exports::household(State(s.state.clone()), admin(&s), HeaderMap::new())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        response.into_body().collect().await.is_ok(),
        "admin export remains usable"
    );
    drop(bodies);
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let response = exports::member(
                State(s.state.clone()),
                member(&s),
                ApiQuery(MemberParams { member_id: None }),
            )
            .await
            .into_response();
            if response.status() != 429 {
                break response;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        response.status(),
        200,
        "body cancellation releases spool admission"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn admission_rejection_does_not_spend_export_step_up(pool: PgPool) {
    use porchlight_api::testing::TestClient;
    use porchlight_core::auth::session::Platform;
    use serde_json::json;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let device = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&device));
    let auth = client
        .request(
            s.lan,
            device.request(
                "POST",
                "/v1/auth/reauth",
                json!({"password":s.admin_password,"action":"household.export"}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(auth.status, 200);
    let token = auth.json()["step_up_token"].as_str().unwrap().to_owned();
    let request = || {
        let mut request = device.request("GET", "/v1/admin/export/household", json!({}), None);
        request
            .headers_mut()
            .insert("porchlight-step-up", token.parse().unwrap());
        request
    };
    let permit = s
        .state
        .export_limits
        .acquire(s.init.household_id, s.init.household_id)
        .unwrap();
    let rejected = client.request(s.lan, request()).await.unwrap();
    assert_eq!(rejected.status, 429);
    drop(permit);
    let retried = client.request(s.lan, request()).await.unwrap();
    assert_eq!(
        retried.status, 200,
        "429 must not consume single-use export authorization"
    );
    assert_eq!(client.request(s.lan, request()).await.unwrap().status, 403);
    assert!(
        s.state
            .export_limits
            .acquire(s.init.household_id, s.init.household_id)
            .is_ok(),
        "auth rejection must release admission"
    );
}
