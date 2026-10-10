mod common;
use common::*;
use porchlight_api::testing::{TestDevice, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
async fn device(s: &TestServer) -> TestDevice {
    s.issue_device(s.init.admin_member_id, Platform::Desktop)
        .await
}
async fn failures(pool: &PgPool) -> i32 {
    sqlx::query_scalar("SELECT failed_attempts FROM porchlight.user_credentials LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn reset_code(s: &TestServer) -> String {
    let user = sqlx::query_scalar("SELECT user_id FROM porchlight.household_members WHERE id=$1")
        .bind(s.init.admin_member_id)
        .fetch_one(&s.state.pool)
        .await
        .unwrap();
    s.reset_code(user).await
}
#[sqlx::test(migrations = "../db/migrations")]
async fn reset_password_flow(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut a = device(&s).await;
    let mut b = device(&s).await;
    login(&s, &mut a).await;
    login(&s, &mut b).await;
    let code = reset_code(&s).await;
    sqlx::query("UPDATE porchlight.user_credentials SET failed_attempts=5,locked_until=now()+interval '1 hour'").execute(&pool).await.unwrap();
    let body = json!({"code":code.to_lowercase().replace('-',""),"new_password":"a freshly reset password"});
    let r = post(&s, &a, "/v1/auth/reset-password", body.clone()).await;
    assert_eq!(r.status, 200);
    problem(&get(&s, &a, "/v1/me").await, 401, "session-invalid");
    problem(&get(&s, &b, "/v1/me").await, 401, "session-invalid");
    assert_eq!(failures(&pool).await, 0);
    let unlocked: bool =
        sqlx::query_scalar("SELECT locked_until IS NULL FROM porchlight.user_credentials")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(unlocked);
    a.session = Some(r.json()["session"]["token"].as_str().unwrap().into());
    assert_eq!(get(&s, &a, "/v1/me").await.status, 200);
    problem(
        &post(&s, &a, "/v1/auth/reset-password", body).await,
        401,
        "reset-code-invalid",
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn reset_code_wrong_device_user(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let a = device(&s).await;
    let code = reset_code(&s).await;
    let uid: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO porchlight.users(household_id,username) VALUES ($1,'other') RETURNING id",
    )
    .bind(s.init.household_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let phc = porchlight_core::auth::password::PasswordHasher::with_params(8, 1, 1)
        .hash("another secure password");
    sqlx::query("INSERT INTO porchlight.user_credentials(household_id,user_id,password_hash) VALUES ($1,$2,$3)").bind(s.init.household_id).bind(uid).bind(phc).execute(&pool).await.unwrap();
    let member: uuid::Uuid=sqlx::query_scalar("INSERT INTO porchlight.household_members(household_id,user_id,role,display_name,color,tzid) VALUES ($1,$2,'user','Other','#123456','America/New_York') RETURNING id").bind(s.init.household_id).bind(uid).fetch_one(&pool).await.unwrap();
    let b = s.issue_device(member, Platform::Desktop).await;
    problem(
        &post(
            &s,
            &b,
            "/v1/auth/reset-password",
            json!({"code":code,"new_password":"a freshly reset password"}),
        )
        .await,
        401,
        "reset-code-invalid",
    );
    assert_eq!(
        post(
            &s,
            &a,
            "/v1/auth/reset-password",
            json!({"code":code,"new_password":"a freshly reset password"})
        )
        .await
        .status,
        200
    );
    let code = reset_code(&s).await;
    for _ in 0..5 {
        problem(
            &post(
                &s,
                &a,
                "/v1/auth/reset-password",
                json!({"code":"invalid","new_password":"a freshly reset password"}),
            )
            .await,
            401,
            "reset-code-invalid",
        );
    }
    problem(
        &post(
            &s,
            &a,
            "/v1/auth/reset-password",
            json!({"code":code,"new_password":"a freshly reset password"}),
        )
        .await,
        401,
        "reset-code-invalid",
    );
    let code = reset_code(&s).await;
    sqlx::query("UPDATE porchlight.password_resets SET expires_at=now()-interval '1 second' WHERE used_at IS NULL").execute(&pool).await.unwrap();
    problem(
        &post(
            &s,
            &a,
            "/v1/auth/reset-password",
            json!({"code":code,"new_password":"a freshly reset password"}),
        )
        .await,
        401,
        "reset-code-invalid",
    );
    s.stop().await;
}
