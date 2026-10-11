use crate::common::problem;
use porchlight_api::testing::{TestOptions, TestServer, trust, wait_blocked_behind};
use porchlight_core::auth::token::hash_token;
use sqlx::PgPool;
use std::time::Duration;

#[sqlx::test(migrations = "../db/migrations")]
async fn ordinary_member_owned_pat_cannot_authenticate_member_routes(pool: PgPool) {
    use porchlight_api::testing::HouseholdFixture;
    use porchlight_core::auth::token::{Token, TokenKind};
    use serde_json::json;
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    for scope in porchlight_api::permissions::PatScope::ALL {
        let token = Token::generate(TokenKind::Pat);
        sqlx::query("INSERT INTO porchlight.api_tokens(household_id,owner_member_id,name,token_hash,scopes,expires_at) VALUES ($1,$2,'Ineligible member',$3,$4,now()+interval '1 day')")
            .bind(f.server.init.household_id).bind(f.users[0].member_id)
            .bind(token.hash().as_slice()).bind(scope.scopes()).execute(&pool).await.unwrap();
        problem(
            &trust::agent(
                &f.server,
                Some(token.expose()),
                "GET",
                "/v1/members",
                json!({}),
                "ineligible-read",
            )
            .await,
            401,
            "unauthenticated",
        );
        for target in [&f.users[0], &f.users[1]] {
            problem(
                &trust::agent(
                    &f.server,
                    Some(token.expose()),
                    "PATCH",
                    &format!("/v1/members/{}", target.member_id),
                    json!({"color":"#123ABC"}),
                    "ineligible-patch",
                )
                .await,
                401,
                "unauthenticated",
            );
        }
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_token_deleted_between_lookup_and_lock(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let issued = trust::enrollment_token(&s, "admin").await;
    let token = issued["token"].as_str().unwrap();
    let (csr, _) = porchlight_pki::generate_csr("device").unwrap();
    // An uncommitted retention deletion remains visible to the initial lookup,
    // but forces the handler's locking recheck to wait until the row is gone.
    let mut deletion = porchlight_db::tx::begin(&pool).await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *deletion)
        .await
        .unwrap();
    sqlx::query("DELETE FROM porchlight.enrollment_tokens WHERE token_hash=$1")
        .bind(hash_token(token).as_slice())
        .execute(&mut *deletion)
        .await
        .unwrap();
    {
        let request = trust::complete(&s, token, &s.admin_password, &csr, "deleted-during-lock");
        tokio::pin!(request);
        tokio::select! {
            response = &mut request => panic!("request did not wait for deletion: {}", response.status),
            blocked = wait_blocked_behind(&pool, pid, 1, Duration::from_secs(15)) => blocked.unwrap(),
        }
        deletion.commit().await.unwrap();
        problem(&request.await, 401, "enrollment-token-invalid");
    }
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key='deleted-during-lock'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(claims, 0, "invalid authentication must not seal a claim");
    problem(
        &trust::complete(&s, token, &s.admin_password, &csr, "deleted-before-lookup").await,
        401,
        "enrollment-token-invalid",
    );
    s.stop().await;
}
