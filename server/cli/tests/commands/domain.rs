use porchlight_api::testing::{TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::Value;
use sqlx::PgPool;
use std::{
    collections::BTreeMap,
    process::{Command, Output, Stdio},
    sync::Arc,
};

async fn output(s: &TestServer, args: &[&str], json_output: bool) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_porch"));
    c.args(args);
    if json_output {
        c.arg("--json");
    }
    c.env("PORCHLIGHT_SOCKET", &s.agent_socket)
        .env("PORCHLIGHT_TOKEN", s.bootstrap_token())
        .env("PORCHLIGHT_PASSWORD", &s.admin_password)
        .stdin(Stdio::null());
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(move || c.output().unwrap()),
    )
    .await
    .unwrap()
    .unwrap()
}
async fn run(s: &TestServer, args: &[&str]) -> (i32, Value) {
    let out = output(s, args, true).await;
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).expect("JSON, including errors"),
    )
}
async fn ok(s: &TestServer, args: &[&str]) -> Value {
    let (code, v) = run(s, args).await;
    assert_eq!(
        code, 0,
        "command {:?} problem {} fields {}",
        args, v["type"], v["fields"]
    );
    v
}
async fn server(pool: &PgPool) -> TestServer {
    TestServer::start_with(
        pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await
}

// Retain whole response shapes, field values, versions, array order and repeated
// ID relationships. Only server-generated identity/time/credential entropy is
// normalized; schedule times and recurrence anchors are deliberately retained.
#[derive(Default)]
struct Goldens {
    families: BTreeMap<String, BTreeMap<String, Value>>,
    ids: BTreeMap<String, String>,
}
impl Goldens {
    fn normalize(&mut self, key: &str, v: &mut Value) {
        match v {
            Value::Object(o) => {
                for (k, v) in o {
                    self.normalize(k, v);
                }
            }
            Value::Array(xs) => {
                for x in xs {
                    self.normalize(key, x);
                }
            }
            Value::String(s) => {
                if uuid::Uuid::parse_str(s).is_ok() {
                    let next = format!("<uuid:{}>", self.ids.len() + 1);
                    *s = self.ids.entry(s.clone()).or_insert(next).clone();
                } else if [
                    "created_at",
                    "updated_at",
                    "deleted_at",
                    "completed_at",
                    "revoked_at",
                    "last_seen_at",
                    "retry_at",
                    "expires_at",
                    "certificate_expires_at",
                ]
                .contains(&key)
                {
                    *s = "<timestamp>".into();
                } else if key == "application_name" && s.starts_with("agent:") {
                    *s = "agent:<label>".into();
                } else if ["fingerprint", "cert_fingerprint", "code", "token"].contains(&key) {
                    *s = "<generated>".into();
                }
            }
            _ => {}
        }
    }
    fn record(&mut self, family: &str, command: &str, v: &Value) {
        let mut v = v.clone();
        self.normalize("", &mut v);
        assert!(
            self.families
                .entry(family.into())
                .or_default()
                .insert(command.into(), v)
                .is_none()
        );
    }
    fn check(self) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens");
        for (family, commands) in self.families {
            let path = root.join(format!("{family}.json"));
            let actual = serde_json::to_value(commands).unwrap();
            // CI never updates by accident. Regenerate only with this explicit opt-in.
            if std::env::var("PORCH_UPDATE_GOLDENS").as_deref() == Ok("1") {
                std::fs::write(
                    &path,
                    format!("{}\n", serde_json::to_string_pretty(&actual).unwrap()),
                )
                .unwrap();
            }
            let expected: Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|_| {
                    panic!("missing {family} golden; explicit PORCH_UPDATE_GOLDENS=1 required")
                }))
                .unwrap();
            assert_eq!(actual, expected, "{family} normalized --json golden");
        }
    }
}
async fn capture(
    g: &mut Goldens,
    s: &TestServer,
    family: &str,
    name: &str,
    args: &[&str],
) -> Value {
    let v = ok(s, args).await;
    g.record(family, name, &v);
    v
}

#[sqlx::test(migrations = "../db/migrations")]
async fn command_families_golden(pool: PgPool) {
    let s = server(&pool).await;
    let mut g = Goldens::default();
    let kid = capture(
        &mut g,
        &s,
        "users",
        "add",
        &[
            "users",
            "add",
            "--display-name",
            "Kid",
            "--role",
            "dependent",
        ],
    )
    .await;
    let mid = kid["id"].as_str().unwrap();
    capture(
        &mut g,
        &s,
        "users",
        "edit",
        &["users", "edit", mid, "--display-name", "Child"],
    )
    .await;
    capture(&mut g, &s, "users", "list", &["users", "list"]).await;
    let item = capture(
        &mut g,
        &s,
        "items",
        "add",
        &[
            "items",
            "add",
            "--title",
            "CLI original",
            "--subject",
            "Child",
            "--assign",
            "admin",
            "--notes",
            "hello",
        ],
    )
    .await;
    let id = item["id"].as_str().unwrap();
    let revisions = capture(&mut g, &s, "history", "revisions", &["revisions", id]).await;
    let rev = revisions["items"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap();
    capture(&mut g, &s, "items", "list", &["items", "list"]).await;
    capture(&mut g, &s, "items", "show", &["items", "show", id]).await;
    capture(
        &mut g,
        &s,
        "items",
        "edit",
        &["items", "edit", id, "--title", "CLI changed"],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items",
        "assign",
        &["items", "assign", id, "Child"],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items",
        "viewers",
        &["items", "viewers", id, "admin"],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items",
        "reminders",
        &["items", "reminders", id, "30", "60"],
    )
    .await;
    capture(
        &mut g,
        &s,
        "history",
        "revert",
        &["revert", id, "--to", rev],
    )
    .await;
    let takeable = ok(
        &s,
        &[
            "items",
            "add",
            "--title",
            "Take me",
            "--visibility",
            "household",
            "--subject",
            "Child",
        ],
    )
    .await;
    let taken = capture(
        &mut g,
        &s,
        "items",
        "take",
        &["items", "take", takeable["id"].as_str().unwrap()],
    )
    .await;
    assert_eq!(taken["assignees"].as_array().unwrap().len(), 1);
    let occs = capture(
        &mut g,
        &s,
        "occurrences",
        "list",
        &["occurrences", "list", id],
    )
    .await;
    let oid = occs["items"][0]["id"].as_str().unwrap();
    for cmd in ["complete", "reopen"] {
        capture(&mut g, &s, "occurrences", cmd, &["occurrences", cmd, oid]).await;
    }
    capture(
        &mut g,
        &s,
        "occurrences",
        "set-status",
        &["occurrences", "set-status", oid, "doing"],
    )
    .await;
    capture(
        &mut g,
        &s,
        "occurrences",
        "move",
        &["occurrences", "move", oid],
    )
    .await;
    capture(
        &mut g,
        &s,
        "occurrences",
        "override",
        &["occurrences", "override", oid, "--due-date", "2027-01-01"],
    )
    .await;
    capture(
        &mut g,
        &s,
        "reads",
        "agenda",
        &[
            "agenda",
            "--member",
            "admin",
            "--from",
            "2026-01-01T00:00:00Z",
            "--to",
            "2027-01-02T00:00:00Z",
        ],
    )
    .await;
    capture(&mut g, &s, "reads", "board", &["board"]).await;
    capture(&mut g, &s, "reads", "search", &["search", "CLI"]).await;
    capture(&mut g, &s, "members", "list", &["members", "list"]).await;
    capture(
        &mut g,
        &s,
        "members",
        "set-tz",
        &["members", "set-tz", "admin", "UTC"],
    )
    .await;
    let device = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    capture(
        &mut g,
        &s,
        "devices",
        "list-filter",
        &["devices", "list", "--member", "admin"],
    )
    .await;
    capture(
        &mut g,
        &s,
        "devices",
        "revoke",
        &["devices", "revoke", &device.device_id.to_string()],
    )
    .await;
    capture(&mut g, &s, "derive", "status", &["derive", "status"]).await;
    capture(&mut g, &s, "derive", "retry", &["derive", "retry", id]).await;
    let (code, error) = run(&s, &["purge", "--item", id, "--yes"]).await;
    assert_eq!(code, 1);
    g.record("purge", "refuse-live", &error);
    capture(&mut g, &s, "items", "rm", &["items", "rm", id]).await;
    capture(
        &mut g,
        &s,
        "purge",
        "item",
        &["purge", "--item", id, "--yes"],
    )
    .await;
    ok(&s, &["items", "rm", takeable["id"].as_str().unwrap()]).await;
    capture(
        &mut g,
        &s,
        "purge",
        "deleted-before",
        &["purge", "--deleted-before", "2099-01-01T00:00:00Z", "--yes"],
    )
    .await;
    capture(&mut g, &s, "users", "rm", &["users", "rm", mid]).await;
    g.check();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn time_recurrence_and_scope_goldens(pool: PgPool) {
    let s = server(&pool).await;
    let mut g = Goldens::default();
    let local = capture(
        &mut g,
        &s,
        "items-flags",
        "local-fixed",
        &[
            "items",
            "add",
            "--title",
            "Local",
            "--start",
            "2026-10-07T10:00",
            "--end",
            "2026-10-07T11:00",
            "--tz",
            "America/New_York",
            "--buffer-before",
            "20",
            "--buffer-after",
            "5",
            "--est",
            "60",
            "--priority",
            "2",
            "--address",
            "Clinic",
        ],
    )
    .await;
    assert_eq!(local["starts_at"], "2026-10-07T14:00:00Z");
    capture(
        &mut g,
        &s,
        "items-flags",
        "rfc3339",
        &[
            "items",
            "add",
            "--title",
            "Offset",
            "--start",
            "2026-10-07T10:00:00+02:00",
            "--end",
            "2026-10-07T11:00:00+02:00",
            "--tz",
            "UTC",
        ],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items-flags",
        "due",
        &[
            "items",
            "add",
            "--title",
            "Due",
            "--due",
            "2026-10-08T12:00",
            "--tz",
            "UTC",
        ],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items-flags",
        "due-rfc3339",
        &[
            "items",
            "add",
            "--title",
            "Due offset",
            "--due",
            "2026-10-08T12:00:00+02:00",
            "--tz",
            "UTC",
        ],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items-flags",
        "floating",
        &[
            "items",
            "add",
            "--title",
            "Floating",
            "--floating",
            "--start",
            "2026-10-07T10:00",
            "--end",
            "2026-10-07T11:00",
        ],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items-flags",
        "all-day",
        &[
            "items",
            "add",
            "--title",
            "All day",
            "--all-day",
            "--tz",
            "UTC",
            "--start",
            "2026-10-07T00:00",
            "--end",
            "2026-10-08T00:00",
        ],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items-flags",
        "completion",
        &[
            "items",
            "add",
            "--title",
            "Completion",
            "--date",
            "2026-10-07",
            "--every",
            "2w",
        ],
    )
    .await;
    for policy in ["skip", "stack", "carry-over"] {
        capture(
            &mut g,
            &s,
            "items-flags",
            policy,
            &[
                "items",
                "add",
                "--title",
                policy,
                "--date",
                "2026-10-07",
                "--rrule",
                "FREQ=DAILY;COUNT=4",
                "--missed",
                policy,
            ],
        )
        .await;
    }
    capture(
        &mut g,
        &s,
        "items-flags",
        "clear",
        &[
            "items",
            "edit",
            local["id"].as_str().unwrap(),
            "--clear",
            "address,est_minutes",
        ],
    )
    .await;
    let series = ok(
        &s,
        &[
            "items",
            "add",
            "--title",
            "Scopes",
            "--floating",
            "--start",
            "2026-10-07T10:00",
            "--end",
            "2026-10-07T11:00",
            "--rrule",
            "FREQ=DAILY;COUNT=4",
        ],
    )
    .await;
    let id = series["id"].as_str().unwrap();
    let occs = ok(&s, &["occurrences", "list", id]).await;
    let oid = occs["items"][1]["id"].as_str().unwrap();
    let ver = occs["items"][1]["version"].to_string();
    capture(
        &mut g,
        &s,
        "items-flags",
        "scope-this",
        &[
            "items",
            "edit",
            id,
            "--scope",
            "this",
            "--occurrence",
            oid,
            "--version",
            &ver,
            "--start",
            "2026-10-08T12:00",
            "--end",
            "2026-10-08T13:00",
        ],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items-flags",
        "scope-all",
        &[
            "items",
            "edit",
            id,
            "--scope",
            "all",
            "--notes",
            "Whole series",
        ],
    )
    .await;
    capture(
        &mut g,
        &s,
        "items-flags",
        "scope-future",
        &[
            "items",
            "edit",
            id,
            "--scope",
            "this-and-future",
            "--occurrence",
            oid,
            "--title",
            "Future",
        ],
    )
    .await;
    g.check();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn release_status_golden(pool: PgPool) {
    let s = server(&pool).await;
    let mut g = Goldens::default();
    let mut status = ok(&s, &["release", "status"]).await;
    assert_eq!(status["running"], porchlight_core::VERSION);
    // The release family reports the build version, not a domain record version.
    status["running"] = serde_json::json!("<running-version>");
    g.record("release", "status", &status);
    g.check();
    let out = output(&s, &["release", "status"], false).await;
    assert!(out.status.success());
    let human = String::from_utf8(out.stdout).unwrap();
    assert!(human.contains(&format!("Running: {}", porchlight_core::VERSION)));
    assert!(human.contains("Update available: false"));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn errors_and_human_tables(pool: PgPool) {
    let s = server(&pool).await;
    let mut g = Goldens::default();
    for (name, args) in [
        ("clap", vec!["items", "show"]),
        ("missing-title", vec!["items", "add"]),
        (
            "start-without-end",
            vec![
                "items",
                "add",
                "--title",
                "Bad",
                "--start",
                "2026-10-07T10:00",
                "--tz",
                "UTC",
            ],
        ),
        (
            "local-no-zone",
            vec![
                "items",
                "add",
                "--title",
                "Bad",
                "--due",
                "2026-10-07T10:00",
            ],
        ),
        (
            "invalid-date",
            vec!["items", "add", "--title", "Bad", "--date", "yesterday"],
        ),
        (
            "invalid-interval",
            vec!["items", "add", "--title", "Bad", "--every", "0w"],
        ),
        (
            "conflicting-recurrence",
            vec![
                "items",
                "add",
                "--title",
                "Bad",
                "--every",
                "1d",
                "--rrule",
                "FREQ=DAILY",
            ],
        ),
        (
            "purge-nonterminal",
            vec!["purge", "--deleted-before", "2099-01-01T00:00:00Z"],
        ),
    ] {
        let (code, v) = run(&s, &args).await;
        assert_eq!(code, 2, "{name}");
        g.record("errors", name, &v);
    }
    let out = output(&s, &["users", "list"], false).await;
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("ID\tTITLE / NAME\tSTATUS / ROLE\tVERSION\n"));
    assert!(text.contains("\tsuper_admin\t"));
    let mut c = Command::new(env!("CARGO_BIN_EXE_porch"));
    c.args(["whoami", "--json"])
        .env_remove("PORCHLIGHT_TOKEN")
        .env("PORCHLIGHT_SOCKET", &s.agent_socket)
        .stdin(Stdio::null());
    let out = tokio::task::spawn_blocking(move || c.output().unwrap())
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    g.record(
        "errors",
        "usage-missing-token",
        &serde_json::from_slice(&out.stdout).unwrap(),
    );
    let missing_socket = s.agent_socket.with_file_name("missing.sock");
    let (code, problem) = run(
        &s,
        &["whoami", "--socket", missing_socket.to_str().unwrap()],
    )
    .await;
    assert_eq!(code, 1);
    g.record("errors", "local-missing-socket", &problem);
    g.check();
    s.stop().await;
}
