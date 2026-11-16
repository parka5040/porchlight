use porchlight_api::testing::{TestOptions, TestServer};
use serde_json::Value;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn sync_cli_accepts_large_pages_and_exhausts_comments(pool: sqlx::PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let id:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'Large collaboration','todo') returning id").bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.comments(household_id,item_id,author_id,body) select $1,$2,$3,repeat('界',9000) from generate_series(1,401)").bind(s.init.household_id).bind(id).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_porch"));
    cmd.args([
        "items",
        "sync",
        &id.to_string(),
        "--include",
        "collaboration",
        "--json",
    ])
    .env("PORCHLIGHT_SOCKET", &s.agent_socket)
    .env("PORCHLIGHT_TOKEN", s.bootstrap_token());
    let output = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        v["comments"].as_array().unwrap().len(),
        401,
        "CLI must exhaust all continuation pages"
    );
    assert_eq!(v["coverage"]["comments_complete"], true);
    assert!(v["comments_next_cursor"].is_null());
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn sync_cli_merges_large_agenda_pages(pool: sqlx::PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let h = s.init.household_id;
    let m = s.init.admin_member_id;
    let id:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,notes,time_mode,tzid,starts_at,ends_at) values($1,$2,'Large CLI agenda','appointment',repeat('界',20000),'fixed','UTC','2026-10-01T09:00Z','2026-10-01T10:00Z') returning id").bind(h).bind(m).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at) select $1,$2,n,'2026-10-01T09:00Z'::timestamptz+n*interval '1 day','2026-10-01T10:00Z'::timestamptz+n*interval '1 day' from generate_series(0,199) n").bind(h).bind(id).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,starts_at,ends_at,busy) select $1,$2,item_id,id,starts_at,ends_at,tstzrange(starts_at,ends_at,'[)') from porchlight.item_occurrences where item_id=$3").bind(h).bind(m).bind(id).execute(&pool).await.unwrap();
    let a = format!("{m}:2026-10-01T00:00:00Z:2028-09-30T00:00:00Z");
    let b = format!("{m}:2026-10-02T00:00:00Z:2028-09-30T00:00:00Z");
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_porch"));
    cmd.args([
        "items",
        "sync",
        &id.to_string(),
        "--agenda",
        &a,
        "--agenda",
        &b,
        "--json",
    ])
    .env("PORCHLIGHT_SOCKET", &s.agent_socket)
    .env("PORCHLIGHT_TOKEN", s.bootstrap_token());
    let output = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["agenda"][0]["items"].as_array().unwrap().len(), 200);
    assert_eq!(v["agenda"][1]["items"].as_array().unwrap().len(), 199);
    for w in v["coverage"]["agenda"].as_array().unwrap() {
        assert_eq!(w["complete"], true);
        assert!(w["next_cursor"].is_null());
    }
    s.stop().await;
}
