use std::process::Command;

#[test]
fn show_rejects_san_flags_before_loading_pki() {
    for args in [["--san-dns", "host.example"], ["--san-ip", "192.168.77.50"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_porchlight"))
            .env_remove("PORCHLIGHT_DATABASE_URL")
            .args(["server-cert", "--show"])
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot be used with"),
            "show must reject SAN options at argument parsing"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_cursor_uses_global_publication_watermark(pool: sqlx::PgPool) {
    use porchlight_api::testing::{TestOptions, TestServer};
    use porchlight_core::auth::session::Platform;
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let device = server
        .session_device(server.init.admin_member_id, Platform::Desktop)
        .await;
    let other: uuid::Uuid = sqlx::query_scalar("INSERT INTO porchlight.households(name,tzid) VALUES ('Other household','UTC') RETURNING id").fetch_one(&pool).await.unwrap();
    // Another household has the latest publication; own data remains unchanged.
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,payload,stream_seq) VALUES ($1,'member.changed','member',$2,'{}',1000000)")
        .bind(other).bind(uuid::Uuid::now_v7()).execute(&pool).await.unwrap();
    let response = crate::common::get(&server, &device, "/v1/snapshot").await;
    assert_eq!(response.status, 200);
    assert_eq!(
        response.json()["cursor"],
        1000000,
        "snapshot must use the stream window's global watermark"
    );
    server.stop().await;
}

#[test]
fn settings_matrix_has_no_fictitious_other_member_target() {
    let rows = porchlight_api::permissions::endpoint_tables()
        .into_iter()
        .flat_map(porchlight_api::permissions::matrix_rows)
        .filter(|row| row.endpoint.path == "/v1/me/settings")
        .collect::<Vec<_>>();
    assert!(!rows.is_empty());
    assert!(
        rows.iter().all(|row| row.is_self),
        "settings has no other-member route"
    );
}
