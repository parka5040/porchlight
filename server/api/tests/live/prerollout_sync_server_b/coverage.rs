//! Differential coverage of real device/agent mutations against delivered TLS frames.
use super::*;
mod prerollout_client_sync_fix_c;
mod proof;
use chrono::{DateTime, Duration as Days, Utc};
use porchlight_api::testing::{TestResponse, agent_request, trust};
use std::collections::{BTreeMap, BTreeSet};

fn now() -> DateTime<Utc> {
    "2027-03-14T06:30:00Z".parse().unwrap()
}
fn id(v: &Value) -> Uuid {
    v["id"].as_str().unwrap().parse().unwrap()
}
struct View {
    bundles: BTreeMap<Uuid, Value>,
    inbox: Value,
    settings: Value,
}
struct Model {
    s: TestServer,
    pool: PgPool,
    devices: [TestDevice; 2],
    members: [Uuid; 2],
    seed: u64,
    random: u64,
    clock: std::sync::Arc<std::sync::Mutex<DateTime<Utc>>>,
    items: Vec<Uuid>,
    list: Uuid,
    entry: Uuid,
    comment: Uuid,
    attachment: Uuid,
    proposals: Vec<Value>,
    seen: [BTreeSet<Uuid>; 2],
}
impl Model {
    fn next(&mut self) -> u64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.random
    }
    async fn get(&self, member: usize, path: &str) -> TestResponse {
        TestClient::new(&self.s.ca_pem, Some(&self.devices[member]))
            .request(
                self.s.lan,
                self.devices[member].request("GET", path, json!({}), None),
            )
            .await
            .unwrap()
    }
    async fn version(&self, table: &str, key: Uuid) -> i32 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select version from porchlight.{table} where id=$1"
        )))
        .bind(key)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
    async fn write(
        &self,
        method: &str,
        path: &str,
        body: Value,
        version: Option<i32>,
        agent: bool,
    ) -> Value {
        let key = Uuid::now_v7().to_string();
        let mut req = if agent {
            trust::req(method, path, body, &key)
        } else {
            self.devices[0].request(method, path, body, Some(&key))
        };
        if let Some(version) = version {
            req.headers_mut()
                .insert("if-match", version.to_string().parse().unwrap());
        }
        let r = if agent {
            agent_request(&self.s.agent_socket, Some(self.s.bootstrap_token()), req).await
        } else {
            TestClient::new(&self.s.ca_pem, Some(&self.devices[0]))
                .request(self.s.lan, req)
                .await
                .unwrap()
        };
        assert!(
            (200..300).contains(&r.status),
            "seed={} {method} {path}: status={} body={}",
            self.seed,
            r.status,
            String::from_utf8_lossy(&r.body)
        );
        if r.body.is_empty() {
            Value::Null
        } else {
            r.json()
        }
    }
    async fn patch_item(&self, n: usize, body: Value, agent: bool) -> Value {
        let key = self.items[n];
        self.write(
            "PATCH",
            &format!("/v1/items/{key}"),
            body,
            Some(self.version("items", key).await),
            agent,
        )
        .await
    }
    async fn relation(&self, kind: &str, body: Value) {
        let key = self.items[1];
        self.write(
            "PUT",
            &format!("/v1/items/{key}/{kind}"),
            body,
            Some(self.version("items", key).await),
            false,
        )
        .await;
    }
    async fn occurrence(&self, n: usize, seq: i64) -> Uuid {
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1 and seq=$2")
            .bind(self.items[n])
            .bind(seq)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn drain(&self) {
        let at = *self.clock.lock().unwrap();
        for _ in 0..20 {
            let n = porchlight_api::derive_worker::run_batch(&self.s.state.pool, at)
                .await
                .unwrap();
            let l = porchlight_api::planning::run_load_batch(&self.s.state.pool, at)
                .await
                .unwrap();
            if n == 0 && l == 0 {
                break;
            }
        }
        porchlight_api::notifier::drain(&self.s.state.pool, at)
            .await
            .unwrap();
        while porchlight_db::outbox::assign_stream_seqs(&self.pool, 500)
            .await
            .unwrap()
            .is_some()
        {}
    }
    async fn view(&self, member: usize) -> View {
        let ids: Vec<Uuid> =
            sqlx::query_scalar("select id from porchlight.items where household_id=$1 order by id")
                .bind(self.s.init.household_id)
                .fetch_all(&self.pool)
                .await
                .unwrap();
        let mut bundles = BTreeMap::new();
        let query = format!(
            "include=collaboration&agenda={}:2027-03-01T00:00:00Z:2028-11-01T00:00:00Z&agenda={}:2027-03-01T00:00:00Z:2028-11-01T00:00:00Z",
            self.members[0], self.members[1]
        );
        for key in ids {
            let response = self
                .get(member, &format!("/v1/items/{key}/sync?{query}"))
                .await;
            match response.status {
                200 => {
                    let bundle = response.json();
                    assert_eq!(
                        bundle["coverage"]["comments_complete"], true,
                        "fixture comments must be complete"
                    );
                    assert!(
                        bundle["coverage"]["agenda"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|scope| scope["complete"] == true),
                        "fixture agenda scopes must be complete"
                    );
                    bundles.insert(key, bundle);
                }
                404 => (),
                other => panic!("seed={} bundle status={other}", self.seed),
            }
        }
        let inbox = self.get(member, "/v1/notifications?limit=200").await;
        assert_eq!(inbox.status, 200);
        assert!(
            inbox.json()["next_cursor"].is_null(),
            "fixture inbox must be complete"
        );
        let mut settings = Vec::new();
        for path in [
            "/v1/me/settings".to_owned(),
            "/v1/me/notification-prefs".to_owned(),
            format!("/v1/members/{}/availability", self.members[member]),
            "/v1/members".to_owned(),
        ] {
            let r = self.get(member, &path).await;
            assert_eq!(r.status, 200, "{path}");
            settings.push(r.json());
        }
        View {
            bundles,
            inbox: inbox.json(),
            settings: json!(settings),
        }
    }
    async fn received(&self, stream: &mut TestStream, barrier: i64) -> (Vec<Value>, bool) {
        let mut frames = Vec::new();
        loop {
            let f = tokio::time::timeout(Duration::from_secs(10), stream.next())
                .await
                .unwrap()
                .unwrap();
            if f.event.as_deref() == Some("resnapshot") {
                assert!(f.id.is_none());
                assert_eq!(
                    serde_json::from_str::<Value>(&f.data).unwrap()["reason"],
                    "projection_changed"
                );
                return (frames, true);
            }
            if let Some(seq) = f.id {
                let seq: i64 = seq.parse().unwrap();
                if seq == barrier {
                    return (frames, false);
                }
                frames.push(serde_json::from_str(&f.data).unwrap());
            }
        }
    }
    async fn step(&mut self, op: &str) {
        proof::checked_step(self, op).await.unwrap();
    }
    async fn mutate(&mut self, op: &str) {
        let title = format!("Seeded task {}", self.next() % 10000);
        match op {
            "create_shared" | "create_private" | "create_task" | "create_task_peer"
            | "horizon_series" => {
                let mut body = if op.contains("task") {
                    json!({"title":title,"kind":"todo","time_mode":"floating","due_date":"2027-03-15","est_minutes":90,"visibility":"household"})
                } else {
                    json!({"title":title,"kind":"appointment","tzid":"America/New_York","starts_at":"2027-03-14T06:30:00Z","ends_at":"2027-03-14T07:30:00Z","visibility":"household"})
                };
                if op == "create_private" {
                    body["visibility"] = json!("private");
                }
                if op == "create_shared" || op.contains("task") {
                    body["assignees"] = json!(self.members);
                }
                if op == "horizon_series" {
                    body["starts_at"] = json!("2027-03-14T08:30:00Z");
                    body["ends_at"] = json!("2027-03-14T09:30:00Z");
                    body["rrule"] = json!("FREQ=DAILY;INTERVAL=60");
                    body["recurrence_mode"] = json!("calendar");
                }
                let r = self
                    .write("POST", "/v1/items", body, None, op == "create_private")
                    .await;
                self.items.push(id(&r));
            }
            "edit" => {
                self.patch_item(0, json!({"notes":title}), true).await;
            }
            "title" => {
                self.patch_item(1, json!({"title":title}), false).await;
            }
            "selected" => {
                self.patch_item(1, json!({"visibility":"selected"}), true)
                    .await;
            }
            "view_gain" => {
                self.relation("viewers", json!({"viewers":[self.members[1]]}))
                    .await
            }
            "view_loss" => self.relation("viewers", json!({"viewers":[]})).await,
            "private" => {
                self.patch_item(1, json!({"visibility":"private"}), false)
                    .await;
            }
            "assign_gain" => {
                self.relation("assignees", json!({"assignees":self.members}))
                    .await
            }
            "assign_loss" => {
                self.relation("assignees", json!({"assignees":[self.members[0]]}))
                    .await
            }
            "time" => {
                self.patch_item(
                    0,
                    json!({"starts_at":"2027-03-14T08:00:00Z","ends_at":"2027-03-14T09:00:00Z"}),
                    true,
                )
                .await;
            }
            "recurrence" => {
                self.patch_item(
                    0,
                    json!({"rrule":"FREQ=DAILY;COUNT=4","recurrence_mode":"calendar"}),
                    false,
                )
                .await;
            }
            "split" => {
                let key = self.items[0];
                let occ = self.occurrence(0, 1).await;
                let r = self
                    .write(
                        "PATCH",
                        &format!("/v1/items/{key}?scope=this_and_future&occurrence_id={occ}"),
                        json!({"title":"Split tail"}),
                        Some(self.version("items", key).await),
                        true,
                    )
                    .await;
                self.items.push(id(&r));
            }
            "board_move" => {
                let occ = self.occurrence(0, 0).await;
                self.write(
                    "PATCH",
                    &format!("/v1/occurrences/{occ}"),
                    json!({"board_move":{}}),
                    Some(self.version("item_occurrences", occ).await),
                    false,
                )
                .await;
            }
            "complete" | "reopen" | "move_detach" => {
                let occ = self.occurrence(0, 0).await;
                let (method, path, body) = if op == "move_detach" {
                    (
                        "PATCH",
                        format!("/v1/occurrences/{occ}"),
                        json!({"override_local_start":"2027-03-14T01:45:00","override_local_end":"2027-03-14T03:45:00"}),
                    )
                } else {
                    ("POST", format!("/v1/occurrences/{occ}/{op}"), json!({}))
                };
                self.write(
                    method,
                    &path,
                    body,
                    Some(self.version("item_occurrences", occ).await),
                    false,
                )
                .await;
            }
            "comment" => {
                self.comment = id(&self
                    .write(
                        "POST",
                        &format!("/v1/items/{}/comments", self.items[0]),
                        json!({"body":"Coverage comment"}),
                        None,
                        false,
                    )
                    .await);
            }
            "comment_edit" => {
                // Comment creation timestamps are DB defaults, unlike injected
                // item clocks. Freeze this operation inside that edit window;
                // both differential reads still use the identical DST clock.
                let created: DateTime<Utc> =
                    sqlx::query_scalar("select created_at from porchlight.comments where id=$1")
                        .bind(self.comment)
                        .fetch_one(&self.pool)
                        .await
                        .unwrap();
                *self.clock.lock().unwrap() = created + Days::minutes(1);
                self.write(
                    "PATCH",
                    &format!("/v1/comments/{}", self.comment),
                    json!({"body":title}),
                    Some(self.version("comments", self.comment).await),
                    false,
                )
                .await;
                *self.clock.lock().unwrap() = now();
            }
            "attachment" => {
                let bytes=b"--sync\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.txt\"\r\nContent-Type: text/plain\r\n\r\nCoverage attachment\r\n--sync--\r\n";
                let mut req = self.devices[0]
                    .request(
                        "POST",
                        &format!("/v1/items/{}/attachments", self.items[0]),
                        json!({}),
                        None,
                    )
                    .map(|_| http_body_util::Full::new(hyper::body::Bytes::from_static(bytes)));
                req.headers_mut().insert(
                    "content-type",
                    "multipart/form-data; boundary=sync".parse().unwrap(),
                );
                let r = TestClient::new(&self.s.ca_pem, Some(&self.devices[0]))
                    .request(self.s.lan, req)
                    .await
                    .unwrap();
                assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
                self.attachment = id(&r.json());
            }
            "attachment_delete" => {
                self.write(
                    "DELETE",
                    &format!("/v1/attachments/{}", self.attachment),
                    json!({}),
                    Some(self.version("attachments", self.attachment).await),
                    false,
                )
                .await;
            }
            "list" => {
                self.list=id(&self.write("POST","/v1/lists",json!({"name":"Inherited checklist","item_id":self.items[0],"mode":"per_occurrence"}),None,false).await);
            }
            "entry" => {
                self.entry = id(&self
                    .write(
                        "POST",
                        &format!("/v1/lists/{}/entries", self.list),
                        json!({"label":title}),
                        None,
                        false,
                    )
                    .await);
            }
            "check" | "uncheck" => {
                let occ = self.occurrence(self.items.len() - 1, 0).await;
                self.write(
                    "POST",
                    &format!("/v1/list-entries/{}/{op}", self.entry),
                    json!({"occurrence_id":occ}),
                    Some(self.version("list_entries", self.entry).await),
                    false,
                )
                .await;
            }
            "list_edit" => {
                self.write(
                    "PATCH",
                    &format!("/v1/lists/{}", self.list),
                    json!({"name":title}),
                    Some(self.version("lists", self.list).await),
                    true,
                )
                .await;
            }
            "reminder" => {
                let key = self.items[0];
                self.write(
                    "PUT",
                    &format!("/v1/items/{key}/reminders"),
                    json!({"reminder_offsets":[10]}),
                    Some(self.version("items", key).await),
                    false,
                )
                .await;
            }
            "availability" | "availability_empty" | "availability_week" => {
                let path = format!("/v1/members/{}/availability", self.members[0]);
                let r = self.get(0, &path).await.json();
                let windows = if op == "availability_empty" {
                    json!([])
                } else if op == "availability_week" {
                    json!((0..7).map(|weekday|json!({"weekday":weekday,"start":"09:00:00","end":"17:00:00"})).collect::<Vec<_>>())
                } else {
                    json!([{"weekday":0,"start":"09:00:00","end":"10:00:00"}])
                };
                self.write(
                    "PUT",
                    &path,
                    json!({"windows":windows}),
                    Some(r["version"].as_i64().unwrap() as i32),
                    false,
                )
                .await;
            }
            "suggest" => {
                self.proposals = self
                    .write("POST", "/v1/schedule/suggest", json!({}), None, false)
                    .await["suggestions"]
                    .as_array()
                    .unwrap()
                    .clone();
                assert!(self.proposals.len() >= 2);
            }
            "accept" | "dismiss" => {
                let p = &self.proposals[usize::from(op == "dismiss")];
                let key = id(p);
                let ver = if op == "accept" {
                    self.version(
                        "item_occurrences",
                        p["occurrence_id"].as_str().unwrap().parse().unwrap(),
                    )
                    .await
                } else {
                    p["version"].as_i64().unwrap() as i32
                };
                self.write(
                    "POST",
                    &format!("/v1/suggestions/{key}/{op}"),
                    json!({}),
                    Some(ver),
                    false,
                )
                .await;
            }
            "member_zone" => {
                let key = self.members[0];
                self.write(
                    "PATCH",
                    &format!("/v1/members/{key}"),
                    json!({"tz_source":"manual","tzid":"America/New_York"}),
                    Some(self.version("household_members", key).await),
                    false,
                )
                .await;
            }
            "settings" => {
                let r = self.get(0, "/v1/me/settings").await.json();
                self.write("PUT","/v1/me/settings",json!({"today_sections":["today"],"scope":"mine","this_week_days":5,"pinned_list_id":null}),Some(r["version"].as_i64().unwrap() as i32),false).await;
            }
            "prefs" => {
                let r = self.get(0, "/v1/me/notification-prefs").await.json();
                let mut prefs = r["prefs"].clone();
                prefs["default_todo_offsets"] = json!([15]);
                self.write(
                    "PUT",
                    "/v1/me/notification-prefs",
                    prefs,
                    Some(r["version"].as_i64().unwrap() as i32),
                    false,
                )
                .await;
            }
            "read" | "read_all" => {
                let inbox = self.get(0, "/v1/notifications").await.json();
                assert!(
                    !inbox["items"].as_array().unwrap().is_empty(),
                    "seed={} inbox fixture must not be vacuous",
                    self.seed
                );
                let path = if op == "read" {
                    format!("/v1/notifications/{}/read", id(&inbox["items"][0]))
                } else {
                    "/v1/notifications/read-all".into()
                };
                self.write("POST", &path, json!({}), None, false).await;
            }
            "horizon" => {
                porchlight_api::scheduler::extend_horizon(
                    &self.s.state.pool,
                    now() + Days::days(120),
                )
                .await
                .unwrap();
            }
            "dst" => {
                porchlight_api::scheduler::refresh_dispositions(
                    &self.s.state.pool,
                    now() + Days::hours(2),
                )
                .await
                .unwrap();
            }
            "role" => {
                let key = self.members[1];
                self.write(
                    "PATCH",
                    &format!("/v1/admin/users/{key}"),
                    json!({"role":"super_admin"}),
                    Some(self.version("household_members", key).await),
                    true,
                )
                .await;
            }
            "delete" => {
                let key = self.items[1];
                self.write(
                    "DELETE",
                    &format!("/v1/items/{key}"),
                    json!({}),
                    Some(self.version("items", key).await),
                    true,
                )
                .await;
            }
            "purge" => {
                self.write(
                    "POST",
                    "/v1/admin/purge",
                    json!({"item_ids":[self.items[1]]}),
                    None,
                    true,
                )
                .await;
            }
            other => proof::mutate_extra(self, other).await,
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn seeded_http_agent_bundle_inbox_settings_coverage(pool: PgPool) {
    let seed = std::env::var("PORCH_SYNC_SEED")
        .ok()
        .map(|v| v.parse().expect("PORCH_SYNC_SEED must be u64"))
        .unwrap_or(0x180b5eed_u64);
    eprintln!("item-sync coverage seed={seed}");
    let clock = std::sync::Arc::new(std::sync::Mutex::new(now()));
    let read_clock = clock.clone();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || *read_clock.lock().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let user = trust::user_named(&pool, &s, "member", "Sam Rivera").await.1;
    let members = [s.init.admin_member_id, user];
    let devices = [
        s.session_device(members[0], Platform::Desktop).await,
        s.session_device(members[1], Platform::Desktop).await,
    ];
    let mut m = Model {
        s,
        pool,
        devices,
        members,
        seed,
        random: seed.max(1),
        clock,
        items: Vec::new(),
        list: Uuid::nil(),
        entry: Uuid::nil(),
        comment: Uuid::nil(),
        attachment: Uuid::nil(),
        proposals: Vec::new(),
        seen: Default::default(),
    };
    for op in [
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
        "split",
        "board_move",
        "complete",
        "reopen",
        "move_detach",
        "comment",
        "comment_edit",
        "attachment",
        "attachment_delete",
        "list",
        "entry",
        "check",
        "uncheck",
        "list_edit",
        "ancestor_private",
        "ancestor_household",
        "ancestor_selected",
        "ancestor_view_gain",
        "ancestor_view_loss",
        "ancestor_delete",
        "ancestor_restore",
        "actor_sources",
        "actor_rename",
        "retention_seed",
        "retention",
        "raw_title",
        "enqueue",
        "reminder",
        "availability",
        "availability_empty",
        "availability_week",
        "suggest",
        "accept",
        "dismiss",
        "member_zone",
        "member_zone_source",
        "settings",
        "prefs",
        "read",
        "read_all",
        "horizon",
        "dst",
    ] {
        eprintln!("seed={seed} mutation={op}");
        m.step(op).await;
    }
    let choices = [
        "title",
        "edit",
        "selected",
        "view_gain",
        "view_loss",
        "private",
        "assign_gain",
        "assign_loss",
    ];
    for _ in 0..12 {
        let op = choices[m.next() as usize % choices.len()];
        eprintln!("seed={seed} generated_mutation={op}");
        m.step(op).await;
    }
    for op in [
        "role",
        "delete",
        "purge",
        "household_zone",
        "member_seed",
        "member_delete",
    ] {
        m.step(op).await;
    }
    m.s.stop().await;
}
