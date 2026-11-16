//! Server-owned read transcripts. No client oracle synthesises responses or invalidations.
use super::*;
use axum::{Extension, body::Body};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use http_body_util::BodyExt;
use porchlight_api::server::{ConnInfo, Transport};
use std::io::{Read, Write};
use tower::ServiceExt;

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
fn key(path: &str) -> String {
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let mut parts: Vec<_> = q.split('&').filter(|p| !p.is_empty()).collect();
    parts.sort_unstable();
    if parts.is_empty() {
        p.into()
    } else {
        format!("{p}?{}", parts.join("&"))
    }
}
async fn record_get(
    m: &Model,
    member: usize,
    path: &str,
    responses: &mut BTreeMap<String, Value>,
) -> Value {
    let r = m.get(member, path).await;
    let body = r.json();
    assert!(
        matches!(r.status, 200 | 403 | 404),
        "GET {path}: {} {body}",
        r.status
    );
    responses.insert(key(path), json!({"status": r.status, "body":body}));
    body
}
async fn pages(m: &Model, member: usize, path: &str, responses: &mut BTreeMap<String, Value>) {
    let mut next = path.to_owned();
    loop {
        let r = record_get(m, member, &next, responses).await;
        let Some(cursor) = r["next_cursor"].as_str() else {
            break;
        };
        next = format!("{path}&cursor={}", encode(cursor));
    }
}

// Capture the actual SSE body, including field order, spaces, ids and delimiters.
// The capable caught_up frame is a real replay barrier, not a fabricated tickle.
async fn frames(m: &Model, member: usize, after: i64) -> String {
    let d = &m.devices[member];
    let router =
        porchlight_api::routes::mtls_router(m.s.state.clone()).layer(Extension(ConnInfo {
            transport: Transport::Lan,
            client_addr: Some("192.0.2.10:12345".parse().unwrap()),
            client_cert_fingerprint: Some(d.fingerprint),
        }));
    let mut req = d
        .request("GET", &format!("/v1/stream?after={after}"), json!({}), None)
        .map(Body::new);
    req.headers_mut()
        .insert("Porchlight-Capabilities", "item-sync-v1".parse().unwrap());
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Ok(data) = frame.into_data() {
            bytes.extend_from_slice(&data);
        }
        let text = std::str::from_utf8(&bytes).unwrap();
        if text.contains("event: caught_up\n") && text.ends_with("\n\n")
            || text.contains("event: resnapshot\n")
        {
            break;
        }
    }
    let wire = String::from_utf8(bytes).unwrap();
    assert!(
        !wire.contains("cursor_expired"),
        "recorder must exercise capable replay, not expired retention"
    );
    wire
}
fn windows(m: &Model, _at: DateTime<Utc>) -> Vec<(Uuid, DateTime<Utc>, DateTime<Utc>)> {
    // Stable retained UI scopes encompass every clock used by this campaign.
    let start = now() - Days::days(180);
    let end = now() + Days::days(120 + 548);
    m.members
        .iter()
        .flat_map(|member| {
            let mut result = Vec::new();
            let mut from = start;
            while from < end {
                let to = (from + Days::days(730)).min(end);
                result.push((*member, from, to));
                from = to;
            }
            result
        })
        .collect()
}
fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
async fn reads(m: &Model, member: usize, at: DateTime<Utc>, wire: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    record_get(m, member, "/v1/snapshot", &mut out).await;
    for scope in ["household", "mine"] {
        pages(
            m,
            member,
            &format!("/v1/board?scope={scope}&limit=200"),
            &mut out,
        )
        .await;
    }
    let retained = windows(m, at);
    // Default full-refresh agenda plus all retained member scopes.
    let from = stamp(at - Days::days(180));
    let to = stamp(at + Days::days(548));
    pages(
        m,
        member,
        &format!(
            "/v1/agenda?from={}&to={}&limit=200",
            encode(&from),
            encode(&to)
        ),
        &mut out,
    )
    .await;
    for (id, from, to) in &retained {
        pages(
            m,
            member,
            &format!(
                "/v1/agenda?from={}&to={}&member_id={id}&limit=200",
                encode(&stamp(*from)),
                encode(&stamp(*to))
            ),
            &mut out,
        )
        .await;
    }
    for path in [
        "/v1/notifications?unread=false&limit=200".to_owned(),
        "/v1/members?limit=200".to_owned(),
        "/v1/me/settings".to_owned(),
        "/v1/me/notification-prefs".to_owned(),
        "/v1/lists?limit=200".to_owned(),
        "/v1/suggestions?limit=200".to_owned(),
    ] {
        pages(m, member, &path, &mut out).await;
    }
    for id in m.members {
        record_get(
            m,
            member,
            &format!("/v1/members/{id}/availability"),
            &mut out,
        )
        .await;
    }
    let agenda = retained
        .iter()
        .map(|(id, from, to)| {
            format!(
                "agenda={}",
                encode(&format!("{id}:{}:{}", stamp(*from), stamp(*to)))
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    for id in &m.items {
        for include in [false, true] {
            let path = format!(
                "/v1/items/{id}/sync?{agenda}{}",
                if include {
                    "&include=collaboration"
                } else {
                    ""
                }
            );
            let mut next = path;
            loop {
                let b = record_get(m, member, &next, &mut out).await;
                let mut query = Vec::new();
                if let Some(cursor) = b["comments_next_cursor"].as_str() {
                    query.push(format!("comments_cursor={}", encode(cursor)));
                }
                if let Some(scopes) = b["coverage"]["agenda"].as_array() {
                    for scope in scopes {
                        if let Some(cursor) = scope["next_cursor"].as_str() {
                            query.push(format!("agenda_cursor={}", encode(cursor)));
                        }
                    }
                }
                if query.is_empty() {
                    break;
                }
                next = format!("/v1/items/{id}/sync?{}", query.join("&"));
            }
        }
    }
    for block in wire.split("\n\n") {
        let Some(data) = block.lines().find_map(|l| l.strip_prefix("data: ")) else {
            continue;
        };
        let f: Value = serde_json::from_str(data).unwrap();
        let Some(id) = f["entity_id"].as_str() else {
            continue;
        };
        if f["entity_type"] == "notification" {
            record_get(m, member, &format!("/v1/notifications?ids={id}"), &mut out).await;
        }
        if f["entity_type"] == "comment"
            && let Some(item) = f["item_id"].as_str()
        {
            record_get(
                m,
                member,
                &format!("/v1/items/{item}/comments?ids={id}"),
                &mut out,
            )
            .await;
        }
    }
    if m.list != Uuid::nil() {
        record_get(m, member, &format!("/v1/lists/{}", m.list), &mut out).await;
        let os: Vec<Uuid> = sqlx::query_scalar(
            "select id from porchlight.item_occurrences where item_id=any($1) order by id",
        )
        .bind(&m.items)
        .fetch_all(&m.pool)
        .await
        .unwrap();
        for o in os {
            let path = format!("/v1/lists/{}?occurrence_id={o}", m.list);
            let r = m.get(member, &path).await;
            if matches!(r.status, 200 | 403 | 404) {
                out.insert(key(&path), json!({"status":r.status,"body":r.json()}));
            }
        }
    }
    out
}

async fn run(pool: PgPool, seed: u64) {
    let mut m = proof::fixture(pool, seed).await;
    // Set the household zone before the initial baseline; subsequent operations use real writers.
    sqlx::query("update porchlight.households set tzid='America/New_York' where id=$1")
        .bind(m.s.init.household_id)
        .execute(&m.pool)
        .await
        .unwrap();
    m.drain().await;
    let mut cursor = porchlight_db::stream::maximum(&m.pool).await.unwrap();
    let mut steps = Vec::new();
    let operations = [
        "initial",
        "create_shared",
        "create_private",
        "create_task",
        "create_task_peer",
        "horizon_series",
        "edit",
        "title",
        "selected",
        "view_gain",
        "view_loss",
        "private",
        "assign_gain",
        "assign_loss",
        "time",
        "recurrence",
        "list",
        "entry",
        "split",
        "check",
        "uncheck",
        "list_edit",
        "board_move",
        "complete",
        "reopen",
        "move_detach",
        "comment",
        "comments_continuation",
        "attachment",
        "attachment_delete",
        "reminder",
        "availability",
        "availability_week",
        "suggest",
        "accept",
        "dismiss",
        "member_zone",
        "actor_rename",
        "settings",
        "prefs",
        "read",
        "read_all",
        "dst",
        "horizon",
        "delete",
        "purge",
    ];
    for op in operations {
        eprintln!("record seed={seed} step={} op={op}", steps.len());
        if op == "dst" {
            *m.clock.lock().unwrap() = now() + Days::hours(2);
        }
        if op == "horizon" {
            *m.clock.lock().unwrap() = now() + Days::days(120);
        }
        if op == "comments_continuation" {
            for _ in 0..202 {
                m.mutate("comment").await;
            }
        } else if op != "initial" {
            m.mutate(op).await;
        }
        m.drain().await;
        let at = *m.clock.lock().unwrap();
        // Mutation timestamps use the database clock; the fixture's domain clock is in
        // the future. Keep replay retention aligned without changing event contents.
        sqlx::query("update porchlight.outbox set created_at=$1")
            .bind(at)
            .execute(&m.pool)
            .await
            .unwrap();
        let mut views = Vec::new();
        for member in 0..2 {
            let wire = if op == "initial" {
                String::new()
            } else {
                frames(&m, member, cursor).await
            };
            let responses = reads(&m, member, at, &wire).await;
            views.push(json!({"member_id":m.members[member],"sse":wire,"responses":responses}));
        }
        cursor = porchlight_db::stream::maximum(&m.pool).await.unwrap();
        steps.push(json!({"operation":op,"now":stamp(at),"windows":windows(&m, at).iter().map(|(id,from,to)| json!({"memberId":id,"from":stamp(*from),"to":stamp(*to)})).collect::<Vec<_>>(),"members":views}));
    }
    let recording = json!({"schema":1,"seed":seed,"steps":steps});
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../client/shared/src/jvmTest/resources/sync-recordings/{seed}.json.gz"
    ));
    if std::env::var_os("PORCHLIGHT_RECORD_SYNC").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut gzip = GzEncoder::new(Vec::new(), Compression::best());
        gzip.write_all(&serde_json::to_vec(&recording).unwrap())
            .unwrap();
        std::fs::write(&path, gzip.finish().unwrap()).unwrap();
    }
    let mut bytes = Vec::new();
    GzDecoder::new(
        std::fs::File::open(&path)
            .expect("missing recording: run PORCHLIGHT_RECORD_SYNC=1 recorder"),
    )
    .read_to_end(&mut bytes)
    .unwrap();
    let expected: Value = serde_json::from_slice(&bytes).unwrap();
    // Identity/time/cursor normalisation is implemented separately from response production.
    if let Some(difference) = first_difference(&canonical(&recording), &canonical(&expected), "") {
        panic!("recording drift seed={seed}: {difference}");
    }
    m.s.stop().await;
}

fn first_difference(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) if a.keys().eq(b.keys()) => a
            .iter()
            .find_map(|(k, v)| first_difference(v, &b[k], &format!("{path}/{k}"))),
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => a
            .iter()
            .zip(b)
            .enumerate()
            .find_map(|(i, (a, b))| first_difference(a, b, &format!("{path}/{i}"))),
        _ if a == b => None,
        _ => Some(format!(
            "{path}: actual={} expected={}",
            a.to_string().chars().take(300).collect::<String>(),
            b.to_string().chars().take(300).collect::<String>()
        )),
    }
}

fn canonical(value: &Value) -> Value {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    // Discover aliases in chronological snapshots, before traversing request keys
    // (which are sorted by random UUID). Preserve relationships everywhere, including
    // URL keys, raw SSE JSON and signed continuation payloads; ignore only the MAC.
    let mut aliases = BTreeMap::<String, String>::new();
    fn visit(v: &Value, aliases: &mut BTreeMap<String, String>, field: &str) -> Value {
        match v {
            Value::Object(obj) => Value::Object(
                obj.iter()
                    .map(|(k, v)| (text(k, aliases), visit(v, aliases, k)))
                    .collect(),
            ),
            Value::Array(rows) => {
                Value::Array(rows.iter().map(|r| visit(r, aliases, field)).collect())
            }
            Value::String(_)
                if matches!(
                    field,
                    "created_at"
                        | "updated_at"
                        | "derived_at"
                        | "released_at"
                        | "first_event_at"
                        | "last_event_at"
                ) =>
            {
                json!("operational-time")
            }
            Value::String(s) if field == "sse" => {
                // SQL UPDATE ... RETURNING may enumerate independent recipients in a
                // different order. Compare the complete per-member event multiset,
                // retaining ready/caught_up cursor bounds; replay still uses raw order.
                let mut events = s
                    .split("\n\n")
                    .filter(|block| !block.is_empty())
                    .map(|block| {
                        let event = block
                            .lines()
                            .find_map(|l| l.strip_prefix("event: "))
                            .unwrap();
                        let data = block
                            .lines()
                            .find_map(|l| l.strip_prefix("data: "))
                            .unwrap();
                        let mut data = visit(&serde_json::from_str(data).unwrap(), aliases, "");
                        if let Some(seq) = data.get_mut("seq") {
                            *seq = json!("position");
                        }
                        format!("{event}:{data}")
                    })
                    .collect::<Vec<_>>();
                events.sort();
                json!(events)
            }
            Value::String(s) => json!(text(s, aliases)),
            _ => v.clone(),
        }
    }
    fn text(s: &str, aliases: &mut BTreeMap<String, String>) -> String {
        if Uuid::parse_str(s).is_ok() {
            let n = aliases.len();
            return aliases
                .entry(s.to_owned())
                .or_insert_with(|| format!("id-{n}"))
                .clone();
        }
        // A continuation is serialized JSON followed by a 32-byte MAC.
        if let Ok(bytes) = URL_SAFE_NO_PAD.decode(s)
            && bytes.len() > 32
            && let Ok(payload) = serde_json::from_slice::<Value>(&bytes[..bytes.len() - 32])
        {
            return format!("cursor:{}", visit(&payload, aliases, ""));
        }
        if s.starts_with('/') {
            let (path, query) = s.split_once('?').unwrap_or((s, ""));
            let path = path
                .split('/')
                .map(|part| text(part, aliases))
                .collect::<Vec<_>>()
                .join("/");
            let mut query = url::form_urlencoded::parse(query.as_bytes())
                .map(|(k, v)| {
                    // Agenda windows contain a member UUID followed by RFC3339 bounds.
                    let v = if k == "agenda" {
                        let (id, rest) = v.split_once(':').unwrap();
                        format!("{}:{rest}", text(id, aliases))
                    } else {
                        text(&v, aliases)
                    };
                    format!("{k}={v}")
                })
                .collect::<Vec<_>>();
            query.sort();
            return if query.is_empty() {
                path
            } else {
                format!("{path}?{}", query.join("&"))
            };
        }
        s.to_owned()
    }
    for step in value["steps"].as_array().unwrap() {
        for member in step["members"].as_array().unwrap() {
            visit(
                &member["responses"]["/v1/snapshot"]["body"],
                &mut aliases,
                "",
            );
        }
    }
    visit(value, &mut aliases, "")
}
#[test]
fn canonical_preserves_routing_and_answers() {
    let id = "01999999-1111-7000-8000-000000000001";
    let wire = format!(
        "event: ready\ndata: {{\"cursor\":1}}\n\nid: 2\nevent: item.changed\ndata: {{\"seq\":2,\"entity_id\":\"{id}\"}}\n\nevent: caught_up\ndata: {{\"cursor\":2}}\n\n"
    );
    let recording = json!({"steps":[{"members":[{"sse":wire,"responses":{
        "/v1/snapshot":{"status":200,"body":{"items":[{"id":id,"title":"Task"}]}}
    }}]}]});
    let renamed: Value = serde_json::from_str(
        &recording
            .to_string()
            .replace(id, "01999999-1111-7000-8000-000000000002"),
    )
    .unwrap();
    assert_eq!(canonical(&recording), canonical(&renamed));
    let mut omitted = recording.clone();
    omitted["steps"][0]["members"][0]["sse"] =
        json!("event: ready\ndata: {\"cursor\":1}\n\nevent: caught_up\ndata: {\"cursor\":2}\n\n");
    assert_ne!(canonical(&recording), canonical(&omitted));
    let mut changed = recording.clone();
    changed["steps"][0]["members"][0]["responses"]["/v1/snapshot"]["body"]["items"][0]["title"] =
        json!("Changed");
    assert_ne!(canonical(&recording), canonical(&changed));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn recorded_seed_1575937(pool: PgPool) {
    run(pool, 0x180c01).await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn recorded_seed_1575938(pool: PgPool) {
    run(pool, 0x180c02).await;
}
