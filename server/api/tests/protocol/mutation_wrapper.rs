use axum::{
    Json, Router,
    body::Body,
    http::Request,
    response::{IntoResponse, Response},
};
use porchlight_api::{
    error::{ApiError, Problem},
    extract::Caller,
    mutation,
    server::{ConnInfo, Transport},
    testing::TestServer,
};
use sqlx::PgPool;
use tower::ServiceExt;
#[derive(serde::Deserialize)]
struct Input {
    mode: String,
}
fn handler<'a>(
    state: &'a porchlight_api::state::AppState,
    caller: Caller,
    tx: &'a mut porchlight_api::idempotency::IdemTx,
    body: Input,
) -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        // State supplies non-SQL services; every SQL await uses the claimed transaction.
        let settings = &state.settings;
        let tx_database: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&mut *tx.tx)
            .await?;
        assert!(!tx_database.is_empty());
        assert!(settings.device_cert_days > 0); // Borrowed state still available after await.
        assert_eq!(caller.actor().request_id, caller.actor().request_id);
        let transaction_request: String =
            sqlx::query_scalar("SELECT current_setting('porchlight.request_id')")
                .fetch_one(&mut *tx.tx)
                .await?;
        assert_eq!(transaction_request, caller.actor().request_id.to_string());
        sqlx::query("UPDATE porchlight.user_credentials SET failed_attempts=failed_attempts+1")
            .execute(&mut *tx.tx)
            .await?;
        match body.mode.as_str() {
            "business" => Err(ApiError::new(Problem::ValidationFailed)),
            "internal" => Err(ApiError::internal("test_wrapper_internal")),
            _ => Ok(Json(serde_json::json!({"request":caller.actor().request_id,"sees_all":caller.sees_all()})).into_response()),
        }
    })
}
#[sqlx::test(migrations = "../db/migrations")]
async fn generic_wrapper_stores_business_errors_replays_and_rolls_back_internal(pool: PgPool) {
    // A single connection is a cheap guard: callback pool SQL cannot escape IdemTx.
    let s = TestServer::start_with(
        &pool,
        porchlight_api::testing::TestOptions {
            db_pool_size: 1,
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
    for agent in [false, true] {
        let conn = ConnInfo {
            transport: if agent {
                Transport::Agent
            } else {
                Transport::Lan
            },
            client_addr: None,
            client_cert_fingerprint: if agent { None } else { Some(d.fingerprint) },
        };
        for (verb, constructor) in [
            ("POST", mutation::post(handler)),
            ("PUT", mutation::put(handler)),
            ("PATCH", mutation::patch(handler)),
            ("DELETE", mutation::delete(handler)),
        ] {
            let router = Router::new()
                .route("/test", constructor)
                .layer(axum::Extension(conn.clone()))
                .with_state(s.state.clone());
            let mut success_body = None;
            let mut business_body = None;
            for (mode, key, status) in [
                ("success", "one", 200),
                ("success", "one", 200),
                ("business", "two", 422),
                ("business", "two", 422),
                ("internal", "three", 500),
            ] {
                let token = if agent {
                    s.bootstrap_token()
                } else {
                    d.session.as_deref().unwrap()
                };
                let r = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    router.clone().oneshot(
                        Request::builder()
                            .method(verb)
                            .uri("/test")
                            .header("authorization", format!("Bearer {token}"))
                            .header("content-type", "application/json")
                            .header("idempotency-key", format!("{verb}-{key}"))
                            .body(Body::from(serde_json::json!({"mode":mode}).to_string()))
                            .unwrap(),
                    ),
                )
                .await
                .expect("callbacks must use IdemTx, never state.pool")
                .unwrap();
                assert_eq!(r.status(), status);
                let bytes = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
                if mode == "business" {
                    if let Some(old) = &business_body {
                        assert_eq!(old, &bytes);
                    } else {
                        business_body = Some(bytes.clone());
                    }
                }
                if mode == "success" {
                    if let Some(old) = &success_body {
                        assert_eq!(old, &bytes);
                    } else {
                        success_body = Some(bytes);
                    }
                }
            }
        }
    }
    let count: i32 = sqlx::query_scalar("SELECT failed_attempts FROM porchlight.user_credentials")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 16);
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key LIKE '%-one' OR idem_key LIKE '%-two' OR idem_key LIKE '%-three'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(claims, 16);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn member_write_limits_include_device_and_address_but_agents_are_unlimited(pool: PgPool) {
    use porchlight_api::{ratelimit::RateLimits, testing::TestOptions};
    let options = TestOptions {
        limits: RateLimits {
            auth_device: 1000,
            auth_addr: 1000,
            write_device: 1,
            write_addr: 1,
        },
        ..TestOptions::default()
    };
    let s = TestServer::start_with(&pool, options).await;
    let a = s
        .session_device(
            s.init.admin_member_id,
            porchlight_core::auth::session::Platform::Desktop,
        )
        .await;
    let b = s
        .session_device(
            s.init.admin_member_id,
            porchlight_core::auth::session::Platform::Desktop,
        )
        .await;
    for (device, addr, agent, expected) in [
        (&a, "127.0.0.1:1234", false, 200),
        (&a, "127.0.0.2:1234", false, 429),
        (&b, "127.0.0.1:1234", false, 429),
        (&a, "127.0.0.1:1234", true, 200),
        (&a, "127.0.0.1:1234", true, 200),
    ] {
        let conn = ConnInfo {
            transport: if agent {
                Transport::Agent
            } else {
                Transport::Lan
            },
            client_addr: Some(addr.parse().unwrap()),
            client_cert_fingerprint: (!agent).then_some(device.fingerprint),
        };
        let router = Router::new()
            .route("/test", mutation::post(handler))
            .layer(axum::Extension(conn))
            .with_state(s.state.clone());
        let token = if agent {
            s.bootstrap_token()
        } else {
            device.session.as_deref().unwrap()
        };
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/test")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .header("idempotency-key", uuid::Uuid::now_v7().to_string())
                    .body(Body::from(r#"{"mode":"success"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == 429 {
            assert!(response.headers().contains_key("retry-after"));
        }
    }
    s.stop().await;
}

#[derive(serde::Deserialize)]
struct LockTarget {
    id: uuid::Uuid,
    mode: String,
}
fn lock_target<'a>(
    _state: &'a porchlight_api::state::AppState,
    _: Caller,
    tx: &'a mut porchlight_api::idempotency::IdemTx,
    body: LockTarget,
) -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        // An effect before the failing statement must roll back with the claim.
        sqlx::query("UPDATE porchlight.user_credentials SET failed_attempts=failed_attempts+1")
            .execute(&mut *tx.tx)
            .await?;
        if body.mode == "business" {
            // Retry-After alone is NOT a transient marker. This business error is sealed.
            return Err(ApiError::new(Problem::Conflict).retry(1));
        }
        if matches!(body.mode.as_str(), "40001" | "40P01") {
            let statement = if body.mode == "40001" {
                "DO $$ BEGIN RAISE EXCEPTION 'test serialization' USING ERRCODE='40001'; END $$"
            } else {
                "DO $$ BEGIN RAISE EXCEPTION 'test deadlock' USING ERRCODE='40P01'; END $$"
            };
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&mut *tx.tx)
                .await?;
        }
        if body.mode == "explicit" {
            // A transient error can also be returned before the transaction is aborted.
            return Err(ApiError::transient_conflict().retry(1));
        }
        // `?` maps 55P03 just like 40001/40P01: no sealing SQL may run afterwards.
        sqlx::query("SELECT id FROM porchlight.devices WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(body.id)
            .fetch_one(&mut *tx.tx)
            .await?;
        Ok(axum::http::StatusCode::NO_CONTENT.into_response())
    })
}
#[sqlx::test(migrations = "../db/migrations")]
async fn domain_nowait_and_retryable_sql_errors_roll_back_claim_and_effects(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let a = s
        .session_device(
            s.init.admin_member_id,
            porchlight_core::auth::session::Platform::Desktop,
        )
        .await;
    let b = s
        .session_device(
            s.init.admin_member_id,
            porchlight_core::auth::session::Platform::Desktop,
        )
        .await;
    let router = Router::new()
        .route("/test", mutation::post(lock_target))
        .layer(axum::Extension(ConnInfo {
            transport: Transport::Lan,
            client_addr: None,
            client_cert_fingerprint: Some(a.fingerprint),
        }))
        .with_state(s.state.clone());
    let request = |mode: &str, key: &str| {
        router.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri("/test")
                .header(
                    "authorization",
                    format!("Bearer {}", a.session.as_deref().unwrap()),
                )
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .body(Body::from(
                    serde_json::json!({"id": b.device_id, "mode":mode}).to_string(),
                ))
                .unwrap(),
        )
    };
    let count = || async {
        let claims: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key LIKE 'transient-%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let effects: i32 =
            sqlx::query_scalar("SELECT failed_attempts FROM porchlight.user_credentials")
                .fetch_one(&pool)
                .await
                .unwrap();
        (claims, effects)
    };
    let mut held = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM porchlight.devices WHERE id=$1 FOR UPDATE")
        .bind(b.device_id)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        request("nowait", "transient-nowait"),
    )
    .await
    .expect("NOWAIT must not wait for lock_timeout")
    .unwrap();
    assert_eq!(r.status(), 409);
    assert_eq!(r.headers()["retry-after"], "1");
    let bytes = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["type"],
        "urn:porchlight:problem:conflict"
    );
    assert_eq!(count().await, (0, 0));
    held.rollback().await.unwrap();
    assert_eq!(
        request("nowait", "transient-nowait")
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(count().await, (1, 1));
    for mode in ["explicit", "40001", "40P01"] {
        let key = format!("transient-{mode}");
        for _ in 0..2 {
            let r = request(mode, &key).await.unwrap();
            assert_eq!(r.status(), 409);
            assert_eq!(r.headers()["retry-after"], "1");
            assert_eq!(count().await, (1, 1));
        }
    }
    let mut old = None;
    for _ in 0..2 {
        let r = request("business", "transient-business").await.unwrap();
        assert_eq!(r.status(), 409);
        assert_eq!(r.headers()["retry-after"], "1");
        let bytes = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
        if let Some(old) = &old {
            assert_eq!(old, &bytes);
        } else {
            old = Some(bytes);
        }
        assert_eq!(count().await, (2, 2));
    }
    s.stop().await;
}

#[derive(serde::Deserialize)]
struct Members {
    ids: Vec<uuid::Uuid>,
}
fn lock_members<'a>(
    _state: &'a porchlight_api::state::AppState,
    barrier: std::sync::Arc<tokio::sync::Barrier>,
    _: Caller,
    tx: &'a mut porchlight_api::idempotency::IdemTx,
    body: Members,
) -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        // Hold the header caller's authorization read on both transactions before R7 locks.
        barrier.wait().await;
        sqlx::query("SELECT id FROM porchlight.household_members WHERE id=ANY($1) ORDER BY id FOR NO KEY UPDATE").bind(body.ids).fetch_all(&mut *tx.tx).await?;
        Ok(axum::http::StatusCode::NO_CONTENT.into_response())
    })
}
#[sqlx::test(migrations = "../db/migrations")]
async fn r18_two_members_lock_both_members_in_r7_order_without_upgrade_deadlock(pool: PgPool) {
    let f = porchlight_api::testing::HouseholdFixture::build(&pool).await;
    for reverse in [false, true] {
        for _ in 0..20 {
            let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
            let request = |u: &porchlight_api::testing::FixtureUser| {
                let conn = ConnInfo {
                    transport: Transport::Lan,
                    client_addr: None,
                    client_cert_fingerprint: Some(u.device.fingerprint),
                };
                let rendezvous = barrier.clone();
                let router = Router::new()
                    .route(
                        "/test",
                        mutation::post(move |state, caller, tx, body| {
                            lock_members(state, rendezvous.clone(), caller, tx, body)
                        }),
                    )
                    .layer(axum::Extension(conn))
                    .with_state(f.server.state.clone());
                let req = Request::builder()
                    .method("POST")
                    .uri("/test")
                    .header(
                        "authorization",
                        format!("Bearer {}", u.device.session.as_deref().unwrap()),
                    )
                    .header("content-type", "application/json")
                    .header("idempotency-key", uuid::Uuid::now_v7().to_string())
                    .body(Body::from(
                        serde_json::json!({"ids":[f.users[0].member_id,f.users[1].member_id]})
                            .to_string(),
                    ))
                    .unwrap();
                router.oneshot(req)
            };
            let (a, b) = if reverse {
                tokio::join!(request(&f.users[1]), request(&f.users[0]))
            } else {
                tokio::join!(request(&f.users[0]), request(&f.users[1]))
            };
            assert_eq!(a.unwrap().status(), 204);
            assert_eq!(b.unwrap().status(), 204);
        }
    }
    f.server.stop().await;
}

#[derive(serde::Deserialize)]
struct WriteItem {
    id: uuid::Uuid,
    title: String,
}
fn write_item<'a>(
    _state: &'a porchlight_api::state::AppState,
    _: Caller,
    tx: &'a mut porchlight_api::idempotency::IdemTx,
    body: WriteItem,
) -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        sqlx::query("UPDATE porchlight.items SET title=$2 WHERE id=$1")
            .bind(body.id)
            .bind(body.title)
            .execute(&mut *tx.tx)
            .await?;
        Ok(axum::http::StatusCode::NO_CONTENT.into_response())
    })
}
#[sqlx::test(migrations = "../db/migrations")]
async fn api_and_raw_agent_attribution_use_the_same_application_name_key(pool: PgPool) {
    use porchlight_db::testing::{Role, connect_as};
    let s = TestServer::start(&pool).await;
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    let item = porchlight_db::testing::insert_appointment(&pool, &h)
        .await
        .unwrap();
    let (_, token) = porchlight_api::testing::trust::seed_pat(&pool, &s, &["write"]).await;
    sqlx::query("UPDATE porchlight.api_tokens SET name='test' WHERE token_hash=$1")
        .bind(porchlight_core::auth::token::hash_token(&token).as_slice())
        .execute(&pool)
        .await
        .unwrap();
    let d = s
        .session_device(
            s.init.admin_member_id,
            porchlight_core::auth::session::Platform::Desktop,
        )
        .await;
    for agent in [true, false] {
        let conn = ConnInfo {
            transport: if agent {
                Transport::Agent
            } else {
                Transport::Lan
            },
            client_addr: None,
            client_cert_fingerprint: (!agent).then_some(d.fingerprint),
        };
        let router = Router::new()
            .route("/test", mutation::post(write_item))
            .layer(axum::Extension(conn))
            .with_state(s.state.clone());
        let bearer = if agent {
            token.as_str()
        } else {
            d.session.as_deref().unwrap()
        };
        let request = Request::builder()
            .method("POST")
            .uri("/test")
            .header("authorization", format!("Bearer {bearer}"))
            .header("content-type", "application/json")
            .header("idempotency-key", uuid::Uuid::now_v7().to_string())
            .body(Body::from(
                serde_json::json!({"id":item,"title":if agent {"agent-api"}else{"member-api"}})
                    .to_string(),
            ))
            .unwrap();
        assert_eq!(router.oneshot(request).await.unwrap().status(), 204);
        for table in ["item_revisions", "task_events"] {
            let query = format!(
                "SELECT actor FROM porchlight.{table} WHERE item_id=$1 ORDER BY created_at DESC,id DESC LIMIT 1"
            );
            // item_revisions uses rev, not id (choose below).
            let query = if table == "item_revisions" {
                "SELECT actor FROM porchlight.item_revisions WHERE item_id=$1 ORDER BY rev DESC LIMIT 1".to_string()
            } else {
                query
            };
            let actor: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
                .bind(item)
                .fetch_one(&pool)
                .await
                .unwrap();
            if agent {
                assert_eq!(actor["application_name"], "agent:test");
                assert!(actor.get("token_id").is_some());
            } else {
                assert!(actor.get("application_name").is_none());
                assert!(actor.get("token_id").is_none());
            }
        }
    }
    let mut conn = connect_as(&pool, Role::Agent, Some("agent:test"))
        .await
        .unwrap();
    sqlx::query("UPDATE porchlight.items SET title='raw-agent' WHERE id=$1")
        .bind(item)
        .execute(&mut conn)
        .await
        .unwrap();
    let actor: serde_json::Value = sqlx::query_scalar(
        "SELECT actor FROM porchlight.item_revisions WHERE item_id=$1 ORDER BY rev DESC LIMIT 1",
    )
    .bind(item)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actor["application_name"], "agent:test");
    s.stop().await;
}
