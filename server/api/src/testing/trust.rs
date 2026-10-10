use super::{TestClient, TestResponse, TestServer, agent_request};
use axum::http::Request;
use http_body_util::Full;
use hyper::body::Bytes;
use porchlight_core::auth::token::{Token, TokenKind};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

pub fn req(method: &str, path: &str, body: Value, key: &str) -> Request<Full<Bytes>> {
    Request::builder()
        .header("porchlight-client-version", porchlight_core::VERSION)
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .body(Full::new(Bytes::from(serde_json::to_vec(&body).unwrap())))
        .unwrap()
}
pub async fn agent(
    s: &TestServer,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Value,
    key: &str,
) -> TestResponse {
    agent_request(&s.agent_socket, token, req(method, path, body, key)).await
}
pub async fn admin(s: &TestServer, method: &str, path: &str, body: Value) -> TestResponse {
    agent(
        s,
        Some(s.bootstrap_token()),
        method,
        path,
        body,
        &Uuid::now_v7().to_string(),
    )
    .await
}
pub async fn enrollment_token(s: &TestServer, username: &str) -> Value {
    let r = admin(
        s,
        "POST",
        "/v1/admin/enrollment-tokens",
        json!({"username":username,"device_label":"Test enrollment"}),
    )
    .await;
    assert_eq!(r.status, 201, "{r:?}");
    r.json()
}
pub async fn complete(
    s: &TestServer,
    token: &str,
    password: &str,
    csr: &str,
    key: &str,
) -> TestResponse {
    TestClient::pinned(s.ca_fingerprint)
        .request(
            s.enroll,
            req(
                "POST",
                "/v1/enrollment/complete",
                json!({"token":token,"password":password,"csr":csr,"platform":"desktop"}),
                key,
            ),
        )
        .await
        .unwrap()
}
pub async fn seed_pat(pool: &PgPool, s: &TestServer, scopes: &[&str]) -> (Uuid, String) {
    let t = Token::generate(TokenKind::Pat);
    let id=sqlx::query_scalar("INSERT INTO porchlight.api_tokens(household_id,owner_member_id,name,token_hash,scopes,expires_at) VALUES ($1,$2,'Test PAT',$3,$4,now()+interval '1 day') RETURNING id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).bind(t.hash().as_slice()).bind(scopes).fetch_one(pool).await.unwrap();
    (id, t.expose().to_owned())
}
pub async fn user(pool: &PgPool, s: &TestServer) -> Uuid {
    user_named(pool, s, "alice", "Alice").await.1
}
pub async fn user_named(pool: &PgPool, s: &TestServer, username: &str, name: &str) -> (Uuid, Uuid) {
    let mut tx = porchlight_db::tx::begin_as(
        pool,
        &porchlight_db::tx::Actor {
            member_id: Some(s.init.admin_member_id),
            device_id: None,
            request_id: Uuid::now_v7(),
            agent_label: None,
            token_id: None,
        },
    )
    .await
    .unwrap();
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO porchlight.users(household_id,username) VALUES ($1,$2) RETURNING id",
    )
    .bind(s.init.household_id)
    .bind(username)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let phc = s.state.hasher.hash(&s.admin_password);
    sqlx::query("INSERT INTO porchlight.user_credentials(household_id,user_id,password_hash) VALUES ($1,$2,$3)").bind(s.init.household_id).bind(uid).bind(phc).execute(&mut *tx).await.unwrap();
    let member = sqlx::query_scalar("INSERT INTO porchlight.household_members(household_id,user_id,role,display_name,color,tzid) VALUES ($1,$2,'user',$3,'#AABBCC','America/New_York') RETURNING id").bind(s.init.household_id).bind(uid).bind(name).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    (uid, member)
}
