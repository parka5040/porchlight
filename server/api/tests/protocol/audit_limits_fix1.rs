use axum::{extract::State, http::HeaderMap, response::IntoResponse};
use porchlight_api::{
    extract::{ApiQuery, Caller},
    routes::exports::{self, MemberParams},
    testing::{TestOptions, TestServer},
};
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn completed_member_spool_does_not_block_admin_export(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    sqlx::query("INSERT INTO porchlight.items(household_id,owner_id,kind,title,notes) SELECT $1,$2,'todo','Export fixture',repeat('x',20000) FROM generate_series(1,32)")
        .bind(s.init.household_id).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    let member = Caller::Member {
        member_id: s.init.admin_member_id,
        user_id: Uuid::nil(),
        role: "super_admin".into(),
        device_id: Uuid::nil(),
        session_id: Uuid::nil(),
        household_id: s.init.household_id,
        client_addr: None,
        request_id: Uuid::now_v7(),
    };
    // Keep the body entirely unpolled: this is stronger backpressure than a full TCP send buffer.
    let unread = exports::member(
        State(s.state.clone()),
        member,
        ApiQuery(MemberParams { member_id: None }),
    )
    .await
    .unwrap();
    assert!(
        unread.headers()["content-length"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 512 * 1024
    );
    let admin = Caller::Agent {
        token_id: Uuid::nil(),
        member_id: s.init.admin_member_id,
        label: "Export fixture".into(),
        scopes: vec!["admin".into()],
        household_id: s.init.household_id,
        request_id: Uuid::now_v7(),
    };
    let response = exports::household(State(s.state.clone()), admin, HeaderMap::new())
        .await
        .into_response();
    assert_eq!(
        response.status(),
        200,
        "completed member spool must release export admission before body polling"
    );
    drop(response);
    drop(unread);
}
