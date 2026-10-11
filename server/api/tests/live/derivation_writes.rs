use axum::{
    Json, Router,
    response::{IntoResponse, Response},
};
use porchlight_api::{
    error::{ApiError, FieldError},
    extract::Caller,
    idempotency::IdemTx,
    mutation,
    server::{ConnInfo, Transport},
    testing::{TestOptions, TestServer},
};
use porchlight_db::{derive, items};
use sqlx::PgPool;
use tower::ServiceExt;
fn create<'a>(
    _state: &'a porchlight_api::state::AppState,
    caller: Caller,
    tx: &'a mut IdemTx,
    mut fields: items::ItemWrite,
) -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        fields.owner_id = caller.member_id();
        let outcome = items::create(
            &mut tx.tx,
            caller.household_id(),
            &fields,
            &Default::default(),
            "2026-10-05T12:00:00Z".parse().unwrap(),
        )
        .await
        .map_err(|e| match e {
            derive::Error::Validation(v) => ApiError::validation(vec![FieldError {
                code: None,
                field: v.field.into(),
                message: v.reason,
            }]),
            derive::Error::Sql(e) => e.into(),
            _ => ApiError::internal("derivation_write_failed"),
        })?;
        Ok((axum::http::StatusCode::CREATED,Json(serde_json::json!({"item_id":outcome.item_id,"conflicts":outcome.derived.conflicts}))).into_response())
    })
}
#[sqlx::test(migrations = "../db/migrations")]
async fn mutation_path_is_synchronous_and_skipped_midnight_input_is_valid(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(
            s.init.admin_member_id,
            porchlight_core::auth::session::Platform::Desktop,
        )
        .await;
    let router = Router::new()
        .route("/test-items", mutation::post(create))
        .layer(axum::Extension(ConnInfo {
            transport: Transport::Lan,
            client_addr: None,
            client_cert_fingerprint: Some(d.fingerprint),
        }))
        .with_state(s.state.clone());
    let fields = items::ItemWrite {
        title: "Created in mutation".into(),
        ..Default::default()
    };
    let response = router
        .clone()
        .oneshot(d.request(
            "POST",
            "/test-items",
            serde_json::to_value(&fields).unwrap(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = value["item_id"].as_str().unwrap();
    let pending: bool = sqlx::query_scalar(
        "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1::uuid",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!pending);
    let skipped_midnight = items::ItemWrite {
        title: "Havana skipped midnight".into(),
        timing: porchlight_core::items::SeriesInput {
            kind: porchlight_core::items::Kind::Appointment,
            tzid: Some("America/Havana".into()),
            starts_at: Some("2026-03-08T05:00:00Z".parse().unwrap()),
            ends_at: Some("2026-03-09T04:00:00Z".parse().unwrap()),
            all_day: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let response = router
        .oneshot(d.request(
            "POST",
            "/test-items",
            serde_json::to_value(skipped_midnight).unwrap(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let local: chrono::NaiveDateTime = sqlx::query_scalar(
        "select local_start from porchlight.item_occurrences where item_id=$1::uuid and seq=0",
    )
    .bind(value["item_id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(local, "2026-03-08T00:00:00".parse().unwrap());
    s.stop().await;
}
