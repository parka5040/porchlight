use axum::{Router, body::to_bytes};
use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test(migrations = "../db/migrations")]
async fn windowed_occurrences_can_include_undated_with_item_acl(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r: Router = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let other: uuid::Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Other','dependent','#ABCDEF','UTC') returning id")
        .bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    for (owner, expected) in [(s.init.admin_member_id, 200), (other, 404)] {
        let item: uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'Undated','todo','private') returning id")
            .bind(s.init.household_id).bind(owner).fetch_one(&pool).await.unwrap();
        sqlx::query(
            "insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0)",
        )
        .bind(s.init.household_id)
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
        for (option, count) in [("", 0), ("&undated=true", 1), ("&undated=false", 0)] {
            let path = format!(
                "/v1/items/{item}/occurrences?from=2026-01-01T00:00:00Z&to=2027-01-01T00:00:00Z{option}"
            );
            let response = r
                .clone()
                .oneshot(d.request("GET", &path, json!({}), None))
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), expected);
            if expected == 200 {
                let v: Value = serde_json::from_slice(
                    &to_bytes(response.into_body(), 1024 * 1024).await.unwrap(),
                )
                .unwrap();
                assert_eq!(v["items"].as_array().unwrap().len(), count, "{option}");
            }
        }
    }
}
