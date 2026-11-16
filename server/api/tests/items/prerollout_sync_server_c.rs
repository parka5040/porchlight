use axum::{
    body::to_bytes,
    extract::{Path, RawQuery, State},
};
use chrono::{DateTime, Duration, Months, NaiveDate, NaiveDateTime, TimeZone, Utc};
use porchlight_api::{
    extract::Caller,
    routes::item_sync,
    testing::{TestOptions, TestServer},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

async fn bundle(s: &TestServer, id: Uuid, query: Option<String>) -> Value {
    let caller = Caller::Agent {
        household_id: s.init.household_id,
        member_id: s.init.admin_member_id,
        token_id: Uuid::nil(),
        label: "Coverage probe".into(),
        scopes: vec!["admin".into()],
        request_id: Uuid::nil(),
    };
    let response = item_sync::get(State(s.state.clone()), caller, Path(id), RawQuery(query))
        .await
        .unwrap();
    serde_json::from_slice(
        &to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

// Independent client implementation: no server anchor/resolution/membership helpers.
fn local_instant(local: NaiveDateTime, zone: chrono_tz::Tz) -> DateTime<Utc> {
    let mut candidate = local;
    loop {
        match zone.from_local_datetime(&candidate) {
            chrono::LocalResult::Single(t) => return t.to_utc(),
            chrono::LocalResult::Ambiguous(a, b) => return a.min(b).to_utc(),
            chrono::LocalResult::None => {
                use chrono::Timelike;
                candidate = candidate
                    .with_second(0)
                    .unwrap()
                    .with_nanosecond(0)
                    .unwrap()
                    + Duration::minutes(1);
            }
        }
    }
}
fn span(s: NaiveDateTime, e: NaiveDateTime, zone: chrono_tz::Tz) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = local_instant(s, zone);
    let end = local_instant(e, zone);
    (start, if end <= start { start + (e - s) } else { end })
}
fn field<T: std::str::FromStr>(row: &Value, name: &str) -> Option<T> {
    row[name].as_str().map(|s| s.parse().ok().unwrap())
}
fn inside(row: &Value, item: &Value, coverage: &Value) -> bool {
    let zone: chrono_tz::Tz = field(coverage, "zone").unwrap();
    let from: DateTime<Utc> = field(coverage, "from").unwrap();
    let to: DateTime<Utc> = field(coverage, "to").unwrap();
    let detached = row["is_detached"] == true;
    let local = |name: &str| -> Option<NaiveDateTime> {
        detached
            .then(|| field(row, &format!("override_{name}")))
            .flatten()
            .or_else(|| field(row, name))
    };
    let (ls, le, ld) = (local("local_start"), local("local_end"), local("local_due"));
    let day: Option<NaiveDate> = detached
        .then(|| field(row, "override_due_date"))
        .flatten()
        .or_else(|| field(row, "due_date"));
    let mut start = field(row, "starts_at");
    let mut end = field(row, "ends_at");
    let mut due = field(row, "due_at");
    if detached
        && item["time_mode"] == "fixed"
        && let Some(fixed_zone) = field::<chrono_tz::Tz>(item, "tzid")
    {
        if let Some((s, e)) = ls.zip(le) {
            (start, end) = {
                let (s, e) = span(s, e, fixed_zone);
                (Some(s), Some(e))
            };
        }
        if let Some(d) = ld {
            due = Some(local_instant(d, fixed_zone));
        }
    }
    let start = start
        .or(due)
        .or_else(|| ls.map(|s| local_instant(s, zone)))
        .or_else(|| ld.map(|d| local_instant(d, zone)))
        .or_else(|| day.map(|d| local_instant(d.and_hms_opt(0, 0, 0).unwrap(), zone)));
    let end = end
        .or_else(|| ls.zip(le).map(|(s, e)| span(s, e, zone).1))
        .or_else(|| le.map(|e| local_instant(e, zone)))
        .or_else(|| {
            day.map(|d| local_instant(d.succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap(), zone))
        });
    start.is_none_or(|s| s < to && end.map_or(s >= from, |e| e > from))
}

#[sqlx::test(migrations = "../db/migrations")]
async fn generated_coverage_rule_matches_bundle_at_dst_boundaries(pool: PgPool) {
    let clock = Arc::new(Mutex::new(
        "2026-10-05T12:00:00Z".parse::<DateTime<Utc>>().unwrap(),
    ));
    let shared = clock.clone();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(move || *shared.lock().unwrap()),
            ..Default::default()
        },
    )
    .await;
    sqlx::query("update porchlight.households set tzid='America/New_York' where id=$1")
        .bind(s.init.household_id)
        .execute(&pool)
        .await
        .unwrap();
    // Member-local date membership is deliberately different from household-local occurrence membership.
    sqlx::query("update porchlight.household_members set tzid='America/Los_Angeles' where id=$1")
        .bind(s.init.admin_member_id)
        .execute(&pool)
        .await
        .unwrap();
    let zone = chrono_tz::America::New_York;
    let mut clocks = Vec::new();
    for boundary in [
        "2026-03-08T06:30:00Z",
        "2026-03-08T07:00:00Z",
        "2026-03-08T07:30:00Z",
        "2026-11-01T05:00:00Z",
        "2026-11-01T05:30:00Z",
        "2026-11-01T06:00:00Z",
        "2026-11-01T06:30:00Z",
        "2026-11-01T07:00:00Z",
    ] {
        let t: DateTime<Utc> = boundary.parse().unwrap();
        clocks.push(t + Duration::days(180)); // Window starts on the transition day.
        clocks.push(t.checked_sub_months(Months::new(18)).unwrap()); // Window ends there.
    }
    let mut items = Vec::new();
    for kind in 0..9 {
        let item: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,tzid,starts_at,ends_at,local_start,local_end,due_at,local_due,due_date,all_day) values($1,$2,'Coverage fixture',$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) returning id")
            .bind(s.init.household_id).bind(s.init.admin_member_id)
            .bind(match kind { 0|1|8 => "appointment", 2|3 => "deadline", _ => "todo" })
            .bind(if kind == 1 || kind == 3 || kind == 8 { "floating" } else { "fixed" })
            .bind(if kind == 0 || kind == 2 { Some("Europe/Berlin") } else { None })
            .bind((kind == 0).then_some("2026-01-01T10:00:00Z".parse::<DateTime<Utc>>().unwrap()))
            .bind((kind == 0).then_some("2026-01-01T11:00:00Z".parse::<DateTime<Utc>>().unwrap()))
            .bind((kind == 1 || kind == 8).then_some("2026-01-01T10:00:00".parse::<NaiveDateTime>().unwrap()))
            .bind((kind == 1 || kind == 8).then_some("2026-01-01T11:00:00".parse::<NaiveDateTime>().unwrap()))
            .bind((kind == 2).then_some("2026-01-01T10:00:00Z".parse::<DateTime<Utc>>().unwrap()))
            .bind((kind == 3).then_some("2026-01-01T10:00:00".parse::<NaiveDateTime>().unwrap()))
            .bind((kind == 4 || kind == 5).then_some("2026-01-01".parse::<NaiveDate>().unwrap()))
            .bind(kind == 4 || kind == 8).fetch_one(&pool).await.unwrap();
        let mut rows = Vec::new();
        for now in &clocks {
            let from = *now - Duration::days(180);
            let to = now.checked_add_months(Months::new(18)).unwrap();
            for bound in [from, to] {
                // Boundary equality, spans ending/starting there, both folds, gap collapse,
                // far generated anchors moved in/out by detached overrides, and undated rows.
                for minute in [-2880, -120, -60, -30, -1, 0, 1, 30, 60, 120, 2880] {
                    let at = bound + Duration::minutes(minute);
                    let local = at.with_timezone(&zone).naive_local();
                    for detached in [false, true] {
                        let mut row = json!({"seq": rows.len()});
                        let generated = if detached {
                            at - Duration::days(1500)
                        } else {
                            at
                        };
                        let generated_local = generated.with_timezone(&zone).naive_local();
                        match kind {
                            0 | 1 => {
                                if kind == 0 {
                                    row["starts_at"] = json!(generated);
                                    row["ends_at"] = json!(generated + Duration::minutes(30));
                                } else {
                                    row["local_start"] = json!(generated_local);
                                    row["local_end"] =
                                        json!(generated_local + Duration::minutes(30));
                                }
                                if detached {
                                    row["override_local_start"] = json!(local);
                                    row["override_local_end"] =
                                        json!(local + Duration::minutes(30));
                                }
                            }
                            2 | 3 => {
                                row[if kind == 2 { "due_at" } else { "local_due" }] = if kind == 2 {
                                    json!(generated)
                                } else {
                                    json!(generated_local)
                                };
                                if detached {
                                    row["override_local_due"] = json!(local);
                                }
                            }
                            4 | 5 => {
                                row["due_date"] = json!(generated_local.date());
                                if detached {
                                    row["override_due_date"] = json!(local.date());
                                }
                            }
                            6 => {}
                            8 => {
                                let start = generated_local.date().and_hms_opt(0, 0, 0).unwrap();
                                row["local_start"] = json!(start);
                                row["local_end"] = json!(start + Duration::days(2));
                                if detached {
                                    let start = local.date().and_hms_opt(0, 0, 0).unwrap();
                                    row["override_local_start"] = json!(start);
                                    row["override_local_end"] = json!(start + Duration::days(1));
                                }
                            }
                            _ => {
                                // Detached anchors moving out of the window must not survive the SQL hull.
                                row["due_date"] = json!(local.date());
                                row["override_due_date"] =
                                    json!((local + Duration::days(1500)).date());
                            }
                        }
                        rows.push(row);
                    }
                }
            }
        }
        if kind < 4 {
            for wall in [
                "2026-03-08T02:15:00",
                "2026-03-08T02:45:00",
                "2026-11-01T01:15:00",
                "2026-11-01T01:45:00",
                "2026-03-29T02:15:00",
                "2026-10-25T02:15:00",
            ] {
                let local: NaiveDateTime = wall.parse().unwrap();
                let mut row = json!({"seq": rows.len()});
                if kind % 2 == 0 {
                    // Fixed detached overrides resolve in the item's zone, not the household zone.
                    if kind == 0 {
                        row["starts_at"] = json!("2020-01-01T00:00:00Z");
                        row["ends_at"] = json!("2020-01-01T01:00:00Z");
                        row["override_local_start"] = json!(local);
                        row["override_local_end"] = json!(local + Duration::minutes(10));
                    } else {
                        row["due_at"] = json!("2020-01-01T00:00:00Z");
                        row["override_local_due"] = json!(local);
                    }
                } else if kind == 1 {
                    row["local_start"] = json!(local);
                    row["local_end"] = json!(local + Duration::minutes(10));
                } else {
                    row["local_due"] = json!(local);
                }
                rows.push(row);
            }
        }
        sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at,due_at,local_start,local_end,local_due,due_date,override_local_start,override_local_end,override_local_due,override_due_date) select $1,$2,r.* from jsonb_to_recordset($3) as r(seq int,starts_at timestamptz,ends_at timestamptz,due_at timestamptz,local_start timestamp,local_end timestamp,local_due timestamp,due_date date,override_local_start timestamp,override_local_end timestamp,override_local_due timestamp,override_due_date date)")
            .bind(s.init.household_id).bind(item).bind(json!(rows)).execute(&pool).await.unwrap();
        let full: Vec<Value> = sqlx::query_scalar(
            "select to_jsonb(o)-'during' from porchlight.item_occurrences o where item_id=$1",
        )
        .bind(item)
        .fetch_all(&pool)
        .await
        .unwrap();
        items.push((item, full));
    }
    for now in clocks {
        *clock.lock().unwrap() = now;
        for (id, full) in &items {
            let b = bundle(&s, *id, None).await;
            let coverage = &b["coverage"]["occurrences"];
            assert_eq!(
                coverage["zone"], "America/New_York",
                "household zone must be explicit"
            );
            for (instant, local) in [("from", "local_from"), ("to", "local_to")] {
                let t: DateTime<Utc> = field(coverage, instant).unwrap();
                assert_eq!(
                    field::<NaiveDateTime>(coverage, local).unwrap(),
                    t.with_timezone(&zone).naive_local()
                );
            }
            let expected: BTreeSet<_> = full
                .iter()
                .filter(|r| inside(r, &b["items"][0], coverage))
                .map(|r| r["id"].as_str().unwrap())
                .collect();
            let got: BTreeSet<_> = b["occurrences"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap())
                .collect();
            assert_eq!(
                got, expected,
                "exact omission coverage for item {id}, clock {now}"
            );
            assert!(
                b["coverage"]["scheduled_reminders"].get("zone").is_none(),
                "reminder fire times use instants only"
            );
        }
    }
    let id = items[0].0;
    let from = "2026-11-01T06:30:00Z";
    let to = "2026-11-02T06:30:00Z";
    let b = bundle(
        &s,
        id,
        Some(format!("agenda={}:{}:{}", s.init.admin_member_id, from, to)),
    )
    .await;
    for window in [&b["coverage"]["agenda"][0], &b["agenda"][0]] {
        assert_eq!(window["zone"], "America/Los_Angeles");
        assert_eq!(window["local_from"], "2026-10-31T23:30:00");
        assert_eq!(window["local_to"], "2026-11-01T22:30:00");
    }
    let dependent: Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Dependent fixture','dependent','#ABCDEF','Asia/Tokyo') returning id")
        .bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    let (id, full) = &items[4];
    for (member, agenda_zone) in [
        (s.init.admin_member_id, chrono_tz::America::Los_Angeles),
        (dependent, zone),
    ] {
        sqlx::query("insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,due_date) select household_id,$1,item_id,id,due_date from porchlight.item_occurrences where item_id=$2")
            .bind(member).bind(id).execute(&pool).await.unwrap();
        for (from, to) in [
            ("2026-03-08T07:30:00Z", "2026-03-09T07:00:00Z"),
            ("2026-11-01T06:30:00Z", "2026-11-02T06:30:00Z"),
            ("2026-03-01T00:00:00Z", "2026-11-04T00:00:00Z"),
        ] {
            let from: DateTime<Utc> = from.parse().unwrap();
            let to: DateTime<Utc> = to.parse().unwrap();
            let expected: BTreeSet<_> = full
                .iter()
                .filter(|r| {
                    let day: NaiveDate = field(r, "due_date").unwrap();
                    // These zones' transitions are not at midnight, so PostgreSQL/core midnight
                    // resolution agrees. The agenda deliberately has its own (stale) date, not overrides.
                    local_instant(day.and_hms_opt(0, 0, 0).unwrap(), agenda_zone) < to
                        && local_instant(
                            day.succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap(),
                            agenda_zone,
                        ) > from
                })
                .map(|r| r["id"].as_str().unwrap().to_owned())
                .collect();
            let mut query = format!("agenda={member}:{}:{}", from.to_rfc3339(), to.to_rfc3339())
                .replace('+', "%2B");
            let mut got = BTreeSet::new();
            let mut pages = 0;
            loop {
                let b = bundle(&s, *id, Some(query)).await;
                let coverage = &b["coverage"]["agenda"][0];
                for window in [coverage, &b["agenda"][0]] {
                    assert_eq!(window["zone"], agenda_zone.name());
                    assert_eq!(
                        field::<NaiveDateTime>(window, "local_from").unwrap(),
                        from.with_timezone(&agenda_zone).naive_local()
                    );
                    assert_eq!(
                        field::<NaiveDateTime>(window, "local_to").unwrap(),
                        to.with_timezone(&agenda_zone).naive_local()
                    );
                }
                for row in b["agenda"][0]["items"].as_array().unwrap() {
                    assert!(got.insert(row["occurrence"]["id"].as_str().unwrap().to_owned()));
                }
                pages += 1;
                if coverage["complete"] == true {
                    break;
                }
                let mut encoded = url::form_urlencoded::Serializer::new(String::new());
                encoded.append_pair("agenda_cursor", coverage["next_cursor"].as_str().unwrap());
                query = encoded.finish();
            }
            assert_eq!(
                got, expected,
                "agenda dates must use the reported member zone"
            );
            if expected.len() > 200 {
                assert!(pages > 1);
            }
        }
    }
    let b = bundle(
        &s,
        *id,
        Some(format!(
            "agenda={}:2026-11-01T00:00:00Z:2026-11-02T00:00:00Z",
            Uuid::nil()
        )),
    )
    .await;
    assert!(b["agenda"][0]["items"].as_array().unwrap().is_empty());
    for window in [&b["coverage"]["agenda"][0], &b["agenda"][0]] {
        assert!(window.get("zone").is_none());
    }
    s.stop().await;
}
