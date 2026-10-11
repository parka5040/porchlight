use porchlight_api::testing::TestServer;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn token_list_shows_expiry_without_credentials(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    sqlx::query("UPDATE porchlight.api_tokens SET expires_at=now()+interval '29 days'")
        .execute(&pool)
        .await
        .unwrap();
    for family in ["token", "tokens"] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_porch"));
        command
            .args(["--json", "--socket"])
            .arg(&s.agent_socket)
            .args([family, "list"])
            .env("PORCHLIGHT_TOKEN", s.bootstrap_token());
        let out = tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap();
        assert!(out.status.success());
        assert!(!String::from_utf8_lossy(&out.stdout).contains(s.bootstrap_token()));
        assert!(!String::from_utf8_lossy(&out.stderr).contains(s.bootstrap_token()));
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(value["tokens"][0]["expires_at"].is_string());
        assert_eq!(value["admin_token_health"]["expiring_soon"], true);
        assert!(value["tokens"][0].get("token_hash").is_none());
    }
    s.stop().await;
}
