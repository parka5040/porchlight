use porchlight_api::testing::TestServer;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    process::{Command, Stdio},
    time::Duration,
};
async fn output(s: &TestServer, args: &[&str], json: bool) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_porch"));
    cmd.args(args);
    if json {
        cmd.arg("--json");
    }
    cmd.env("PORCHLIGHT_SOCKET", &s.agent_socket)
        .env("PORCHLIGHT_TOKEN", s.bootstrap_token())
        .stdin(Stdio::null());
    tokio::time::timeout(
        Duration::from_secs(25),
        tokio::task::spawn_blocking(move || cmd.output().unwrap()),
    )
    .await
    .unwrap()
    .unwrap()
}
async fn run(s: &TestServer, args: &[&str]) -> Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_porch"));
    cmd.args(args)
        .arg("--json")
        .env("PORCHLIGHT_SOCKET", &s.agent_socket)
        .env("PORCHLIGHT_TOKEN", s.bootstrap_token())
        .stdin(Stdio::null());
    let result = tokio::time::timeout(
        Duration::from_secs(25),
        tokio::task::spawn_blocking(move || cmd.output().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        result.status.success(),
        "CLI status {:?}",
        result.status.code()
    );
    serde_json::from_slice(&result.stdout).unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn lists_cli_json_and_got_it_lifecycle(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let list = run(&s, &["lists", "add", "Groceries"]).await;
    let id = list["id"].as_str().unwrap();
    let entry = run(
        &s,
        &[
            "lists",
            "add-entry",
            id,
            "Oats",
            "--qty",
            "2",
            "--unit",
            "bags",
        ],
    )
    .await;
    let eid = entry["id"].as_str().unwrap();
    assert_eq!(entry["qty"], 2.0);
    let checked = run(&s, &["lists", "check", eid]).await;
    assert!(checked["checked_at"].is_string());
    let shown = run(&s, &["lists", "show", id]).await;
    assert_eq!(shown["got_it"].as_array().unwrap().len(), 1);
    let second = run(&s, &["lists", "add-entry", id, "Bread"]).await;
    let before_edit = run(&s, &["lists", "show", id]).await;
    run(
        &s,
        &[
            "lists",
            "edit-entry",
            second["id"].as_str().unwrap(),
            "--list-id",
            id,
            "--label",
            "Sourdough",
        ],
    )
    .await;
    let first = run(
        &s,
        &[
            "lists",
            "add-entry",
            id,
            "Yogurt",
            "--before-id",
            second["id"].as_str().unwrap(),
        ],
    )
    .await;
    run(
        &s,
        &[
            "lists",
            "edit-entry",
            first["id"].as_str().unwrap(),
            "--list-id",
            id,
            "--label",
            "Greek yogurt",
        ],
    )
    .await;
    let edited = run(&s, &["lists", "show", id]).await;
    assert_eq!(
        edited["entries"][0]["id"], first["id"],
        "label-only edit must preserve order"
    );
    assert_eq!(
        edited["got_it"][0]["position"],
        before_edit["got_it"][0]["position"]
    );
    run(
        &s,
        &[
            "lists",
            "rm-entry",
            first["id"].as_str().unwrap(),
            "--list-id",
            id,
        ],
    )
    .await;
    run(
        &s,
        &[
            "lists",
            "rm-entry",
            second["id"].as_str().unwrap(),
            "--list-id",
            id,
        ],
    )
    .await;
    run(&s, &["lists", "uncheck", eid]).await;
    run(&s, &["lists", "check", eid]).await;
    let clear = run(&s, &["lists", "clear", id]).await;
    assert_eq!(clear["got_it"], json!([]));
    let page = run(&s, &["lists", "list", "--limit", "1"]).await;
    assert_eq!(page["items"][0]["name"], "Groceries");
    let out = run(&s, &["lists", "rm", id]).await;
    assert!(out["deleted_at"].is_string());
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn names_human_golden_edit_fields_occurrence_and_conflict_exit(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let empty = output(&s, &["lists", "list"], false).await;
    assert!(empty.status.success());
    assert_eq!(
        String::from_utf8(empty.stdout).unwrap(),
        include_str!("../fixtures/lists-empty-human.json")
    );
    let l = run(&s, &["lists", "add", "groceries"]).await;
    let lid = l["id"].as_str().unwrap();
    let e = run(
        &s,
        &["lists", "add-entry", "groceries", "Oat milk", "--qty", "2"],
    )
    .await;
    let eid = e["id"].as_str().unwrap();
    let edited = run(
        &s,
        &[
            "lists",
            "edit-entry",
            eid,
            "--list-id",
            lid,
            "--qty",
            "3",
            "--unit",
            "cartons",
            "--notes",
            "unsweetened",
            "--url",
            "https://example.invalid/milk",
        ],
    )
    .await;
    assert_eq!(edited["qty"], 3.0);
    assert_eq!(edited["notes"], "unsweetened");
    let checked = run(&s, &["lists", "check", eid, "--list-id", lid]).await;
    assert!(checked["checked_at"].is_string());
    let conflict = output(
        &s,
        &["lists", "uncheck", eid, "--list-id", lid, "--version", "0"],
        true,
    )
    .await;
    assert_eq!(conflict.status.code(), Some(1));
    let problem: Value = serde_json::from_slice(&conflict.stdout).unwrap();
    assert_eq!(problem["type"], "urn:porchlight:problem:version-conflict");
    let item=api(&s,"POST","/v1/items",json!({"title":"Meds","kind":"deadline","time_mode":"fixed","due_at":"2026-10-05T12:00:00Z","tzid":"UTC","rrule":"FREQ=DAILY;COUNT=3","recurrence_mode":"calendar"}),None).await;
    let il = run(
        &s,
        &[
            "lists",
            "add",
            "doses",
            "--item-id",
            item["id"].as_str().unwrap(),
            "--mode",
            "per-occurrence",
        ],
    )
    .await;
    let ie = run(
        &s,
        &["lists", "add-entry", il["id"].as_str().unwrap(), "dose"],
    )
    .await;
    let occ: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM porchlight.item_occurrences WHERE item_id=$1 ORDER BY seq LIMIT 1",
    )
    .bind(uuid::Uuid::parse_str(item["id"].as_str().unwrap()).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let checked = run(
        &s,
        &[
            "lists",
            "check",
            ie["id"].as_str().unwrap(),
            "--list-id",
            il["id"].as_str().unwrap(),
            "--occurrence-id",
            &occ.to_string(),
        ],
    )
    .await;
    assert!(checked["checked_at"].is_string());
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn cli_list_skips_deleted_parent(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let item = api(&s, "POST", "/v1/items", json!({"title":"orphan"}), None).await;
    run(
        &s,
        &[
            "lists",
            "add",
            "orphan",
            "--item-id",
            item["id"].as_str().unwrap(),
        ],
    )
    .await;
    api(
        &s,
        "DELETE",
        &format!("/v1/items/{}", item["id"].as_str().unwrap()),
        json!({}),
        Some(&item["version"]),
    )
    .await;
    let page = run(&s, &["lists", "list"]).await;
    assert_eq!(page["items"], json!([]));
    let page = run(&s, &["lists", "list", "--include-orphaned"]).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let list = &page["items"][0];
    run(
        &s,
        &[
            "lists",
            "rm",
            list["id"].as_str().unwrap(),
            "--version",
            &list["version"].to_string(),
        ],
    )
    .await;
    assert!(
        run(&s, &["lists", "list", "--include-orphaned"]).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    s.stop().await;
}

async fn api(
    s: &TestServer,
    method: &str,
    path: &str,
    body: Value,
    version: Option<&Value>,
) -> Value {
    let d = s
        .session_device(
            s.init.admin_member_id,
            porchlight_core::auth::session::Platform::Desktop,
        )
        .await;
    let mut req = d.request(method, path, body, None);
    if let Some(v) = version {
        req.headers_mut()
            .insert("if-match", v.to_string().parse().unwrap());
    }
    let r = porchlight_api::testing::agent_request(&s.agent_socket, Some(s.bootstrap_token()), req)
        .await;
    assert!(r.status < 300, "API fixture status {}", r.status);
    r.json()
}
