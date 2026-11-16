use porchlight_api::testing::{TestOptions, TestServer};
use serde_json::{Value, json};
use sqlx::PgPool;

fn normalize(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if ["created_at", "updated_at"].contains(&key.as_str()) && !value.is_null() {
                    *value = json!("<timestamp>");
                } else {
                    normalize(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize(value);
            }
        }
        Value::String(value) if uuid::Uuid::parse_str(value).is_ok() => *value = "<uuid>".into(),
        _ => {}
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn domain_item_sync_golden(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let id: uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'CLI sync fixture','todo') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    let member = s.init.admin_member_id;
    let agenda = format!("{member}:2026-10-01T00:00:00Z:2026-11-01T00:00:00Z");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_porch"));
    command
        .args([
            "items",
            "sync",
            &id.to_string(),
            "--agenda",
            &agenda,
            "--include",
            "collaboration",
            "--json",
        ])
        .env("PORCHLIGHT_SOCKET", &s.agent_socket)
        .env("PORCHLIGHT_TOKEN", s.bootstrap_token());
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(move || command.output().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
    normalize(&mut actual);
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/item-sync.json");
    if std::env::var("PORCH_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&actual).unwrap()),
        )
        .unwrap();
    }
    let expected: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("explicit golden update required"))
            .unwrap();
    assert_eq!(actual, expected);
    s.stop().await;
}
