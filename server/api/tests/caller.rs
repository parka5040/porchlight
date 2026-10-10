use porchlight_api::{
    extract::Caller,
    testing::{TestClient, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
#[test]
fn only_agents_see_all() {
    let id = uuid::Uuid::now_v7();
    let member = Caller::Member {
        member_id: id,
        user_id: id,
        role: "super_admin".into(),
        device_id: id,
        session_id: id,
        household_id: id,
        client_addr: None,
        request_id: id,
    };
    assert!(!member.sees_all());
    assert_eq!(member.actor().device_id, Some(id));
    let agent = Caller::Agent {
        token_id: id,
        member_id: id,
        label: "test".into(),
        scopes: vec!["read".into()],
        household_id: id,
        request_id: id,
    };
    assert!(agent.sees_all());
    assert_eq!(agent.actor().device_id, None);
    assert!(agent.require_admin().is_err());
}
#[sqlx::test(migrations = "../db/migrations")]
async fn role_change_during_body_read_does_not_forbid_terminal_operations(pool: PgPool) {
    use axum::{body::Body, response::IntoResponse};
    use tower::ServiceExt;
    let s = TestServer::start(&pool).await;
    for logout in [true, false] {
        sqlx::query("UPDATE porchlight.household_members SET role='super_admin' WHERE id=$1")
            .bind(s.init.admin_member_id)
            .execute(&pool)
            .await
            .unwrap();
        let d = s
            .session_device(s.init.admin_member_id, Platform::Desktop)
            .await;
        let reauth = TestClient::new(&s.ca_pem, Some(&d))
            .request(
                s.lan,
                d.request(
                    "POST",
                    "/v1/auth/reauth",
                    json!({"password":s.admin_password,"action":"device.revoke"}),
                    None,
                ),
            )
            .await
            .unwrap();
        assert_eq!(reauth.status, 200);
        let stepup = reauth.json()["step_up_token"].as_str().unwrap().to_owned();
        let path = if logout {
            "/v1/auth/logout".to_owned()
        } else {
            format!(
                "/v1/devices/{}/revoke",
                d.device_id.to_string().to_uppercase()
            )
        };
        let (mut parts, _) = d.request("POST", &path, json!({}), None).into_parts();
        parts
            .headers
            .insert("porchlight-step-up", stepup.parse().unwrap());
        let (send, receive) =
            tokio::sync::mpsc::channel::<Result<hyper::body::Bytes, std::convert::Infallible>>(1);
        let request = axum::http::Request::from_parts(
            parts,
            Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receive)),
        );
        let conn = porchlight_api::server::ConnInfo {
            transport: porchlight_api::server::Transport::Lan,
            client_addr: None,
            client_cert_fingerprint: Some(d.fingerprint),
        };
        let router =
            porchlight_api::routes::mtls_router(s.state.clone()).layer(axum::Extension(conn));
        let started = s.state.mutation_body_started.notified();
        tokio::pin!(started);
        started.as_mut().enable();
        let task = tokio::spawn(router.oneshot(request));
        tokio::time::timeout(std::time::Duration::from_secs(2), started)
            .await
            .unwrap();
        sqlx::query("UPDATE porchlight.household_members SET role='user' WHERE id=$1")
            .bind(s.init.admin_member_id)
            .execute(&pool)
            .await
            .unwrap();
        send.send(Ok(hyper::body::Bytes::from_static(b"{}")))
            .await
            .unwrap();
        drop(send);
        let response = task.await.unwrap().unwrap().into_response();
        assert_eq!(response.status(), if logout { 204 } else { 200 });
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn member_deleted_during_body_read_is_401_not_side_query_500(pool: PgPool) {
    use axum::{body::Body, response::IntoResponse};
    use tower::ServiceExt;
    fn callback<'a>(
        _: &'a porchlight_api::state::AppState,
        _: Caller,
        _: &'a mut porchlight_api::idempotency::IdemTx,
        _: serde_json::Value,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<axum::response::Response, porchlight_api::error::ApiError>,
    > {
        Box::pin(async { Ok(axum::http::StatusCode::NO_CONTENT.into_response()) })
    }
    let f = porchlight_api::testing::HouseholdFixture::build(&pool).await;
    let u = &f.users[0];
    let router = axum::Router::new()
        .route("/test", porchlight_api::mutation::post(callback))
        .layer(axum::Extension(porchlight_api::server::ConnInfo {
            transport: porchlight_api::server::Transport::Lan,
            client_addr: None,
            client_cert_fingerprint: Some(u.device.fingerprint),
        }))
        .with_state(f.server.state.clone());
    let (parts, _) = u
        .device
        .request("POST", "/test", json!({}), None)
        .into_parts();
    let (send, receive) =
        tokio::sync::mpsc::channel::<Result<hyper::body::Bytes, std::convert::Infallible>>(1);
    let req = axum::http::Request::from_parts(
        parts,
        Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receive)),
    );
    let state = f.server.state.clone();
    let started = state.mutation_body_started.notified();
    tokio::pin!(started);
    started.as_mut().enable();
    let task = tokio::spawn(router.oneshot(req));
    tokio::time::timeout(std::time::Duration::from_secs(2), started)
        .await
        .unwrap();
    sqlx::query("UPDATE porchlight.household_members SET deleted_at=now() WHERE id=$1")
        .bind(u.member_id)
        .execute(&pool)
        .await
        .unwrap();
    send.send(Ok(hyper::body::Bytes::from_static(b"{}")))
        .await
        .unwrap();
    drop(send);
    assert_eq!(task.await.unwrap().unwrap().status(), 401);
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn registered_self_pat_revoke_survives_scope_change_after_header_auth(pool: PgPool) {
    use porchlight_api::testing::trust;
    use tower::ServiceExt;
    let s = TestServer::start(&pool).await;
    let (id, token) = trust::seed_pat(&pool, &s, &["admin"]).await;
    let path = format!("/v1/admin/tokens/{}/revoke", id.to_string().to_uppercase());
    let mut request = trust::req("POST", &path, json!({}), "self-pat");
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    let (parts, _) = request.into_parts();
    let (send, receive) =
        tokio::sync::mpsc::channel::<Result<hyper::body::Bytes, std::convert::Infallible>>(1);
    let req = axum::http::Request::from_parts(
        parts,
        axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receive)),
    );
    let router = porchlight_api::routes::agent_router(s.state.clone()).layer(axum::Extension(
        porchlight_api::server::ConnInfo {
            transport: porchlight_api::server::Transport::Agent,
            client_addr: None,
            client_cert_fingerprint: None,
        },
    ));
    {
        let started = s.state.mutation_body_started.notified();
        tokio::pin!(started);
        started.as_mut().enable();
        let task = tokio::spawn(router.oneshot(req));
        tokio::time::timeout(std::time::Duration::from_secs(2), started)
            .await
            .unwrap();
        sqlx::query("UPDATE porchlight.api_tokens SET scopes=ARRAY['write'] WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        send.send(Ok(hyper::body::Bytes::from_static(b"{}")))
            .await
            .unwrap();
        drop(send);
        assert_eq!(task.await.unwrap().unwrap().status(), 200);
    }
    let b = trust::agent(&s, Some(&token), "POST", &path, json!({}), "self-pat").await;
    assert_eq!(b.status, 401);
    s.stop().await;
}
