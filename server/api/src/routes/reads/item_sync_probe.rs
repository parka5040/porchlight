use super::*;
use crate::{
    routes,
    server::{ConnInfo, Transport},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tower::ServiceExt;

fn rows(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn digest(value: &Value) -> Vec<u8> {
    Sha256::digest(serde_json::to_vec(value).unwrap()).to_vec()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn bounded_bundle_cost_and_continuation_equivalence(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let h = s.init.household_id;
    let m = s.init.admin_member_id;
    let id:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,kind,title,notes,time_mode,tzid,starts_at,ends_at) values($1,$2,'appointment','Large agenda',repeat('界',20000),'fixed','UTC','2026-10-01T09:00Z','2026-10-01T10:00Z') returning id").bind(h).bind(m).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at) select $1,$2,n,'2026-10-01T09:00Z'::timestamptz+n*interval '1 day','2026-10-01T10:00Z'::timestamptz+n*interval '1 day' from generate_series(0,719) n").bind(h).bind(id).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,starts_at,ends_at,busy) select $1,$2,item_id,id,starts_at,ends_at,tstzrange(starts_at,ends_at,'[)') from porchlight.item_occurrences where item_id=$3").bind(h).bind(m).bind(id).execute(&pool).await.unwrap();
    // A second occurrence in the same interval exercises conflict decoration.
    let blocker:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,kind,title) values($1,$2,'todo','Conflict fixture') returning id").bind(h).bind(m).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at) values($1,$2,0,'2026-10-01T09:00Z','2026-10-01T10:00Z')").bind(h).bind(blocker).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,starts_at,ends_at,busy) select $1,$2,item_id,id,starts_at,ends_at,tstzrange(starts_at,ends_at,'[)') from porchlight.item_occurrences where item_id=$3").bind(h).bind(m).bind(blocker).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.comments(household_id,item_id,author_id,body) select $1,$2,$3,repeat('界',9000) from generate_series(1,401)").bind(h).bind(id).bind(m).execute(&pool).await.unwrap();
    let d = s
        .session_device(m, porchlight_core::auth::session::Platform::Desktop)
        .await;
    let app = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let from: DateTime<Utc> = "2026-10-01T00:00:00Z".parse().unwrap();
    let mut windows = Vec::new();
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("include", "collaboration");
    for n in [0, 0, 1, 2, 3, 4, 5, 6] {
        let f = from + Duration::days(n);
        let t = f + Duration::days(730);
        query.append_pair("agenda", &format!("{m}:{f}:{t}"));
        if !windows.contains(&(f, t)) {
            windows.push((f, t));
        }
    }
    let path = format!("/v1/items/{id}/sync?{}", query.finish());
    #[cfg(porchlight_snapshot_probe)]
    let base = super::super::alloc_probe::start();
    let mut held = Vec::new();
    for _ in 0..4 {
        let r = app
            .clone()
            .oneshot(d.request("GET", &path, json!({}), None))
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        held.push(r);
    }
    #[cfg(porchlight_snapshot_probe)]
    let peak = super::super::alloc_probe::peak(base);
    #[cfg(not(porchlight_snapshot_probe))]
    let peak = 0usize;
    println!("ITEM_SYNC held4 peak={peak} bound={}", 44 * 1024 * 1024);
    assert!(
        peak <= 44 * 1024 * 1024,
        "four held pages exceeded bounded writer estimate"
    );
    assert_eq!(
        app.clone()
            .oneshot(d.request("GET", &path, json!({}), None))
            .await
            .unwrap()
            .status(),
        429
    );
    let mut first = None;
    for r in held {
        let b = body(r).await;
        assert!(b.len() <= crate::json_document::SYNC_BUDGET);
        println!("ITEM_SYNC identity_page_bytes={}", b.len());
        if first.is_none() {
            first = Some(serde_json::from_slice::<Value>(&b).unwrap());
        }
    }
    let mut page = first.unwrap();
    assert_eq!(rows(&page["agenda"]).len(), 7);
    let mut actual: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    let mut comments = Vec::new();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages < 100);
        let count: usize = rows(&page["agenda"])
            .iter()
            .map(|w| w["items"].as_array().unwrap().len())
            .sum();
        assert!(count <= 200);
        assert!(rows(&page["comments"]).len() <= 200);
        comments.extend(rows(&page["comments"]).iter().map(digest));
        for w in rows(&page["agenda"]) {
            actual
                .entry(w["from"].as_str().unwrap().into())
                .or_default()
                .extend(w["items"].as_array().unwrap().iter().map(digest));
        }
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        let mut more = false;
        for w in page["coverage"]["agenda"].as_array().unwrap() {
            if let Some(c) = w["next_cursor"].as_str() {
                query.append_pair("agenda_cursor", c);
                more = true;
                assert_eq!(w["complete"], false);
            } else {
                assert_eq!(w["complete"], true);
            }
        }
        if let Some(c) = page["comments_next_cursor"].as_str() {
            query.append_pair("comments_cursor", c);
            more = true;
        }
        if !more {
            break;
        }
        let path = format!("/v1/items/{id}/sync?{}", query.finish());
        let r = app
            .clone()
            .oneshot(d.request("GET", &path, json!({}), None))
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let b = body(r).await;
        assert!(b.len() <= crate::json_document::SYNC_BUDGET);
        page = serde_json::from_slice(&b).unwrap();
        assert_eq!(page["coverage"]["continued"], true);
        assert_eq!(page["coverage"]["item_sections"], false);
        for key in [
            "items",
            "occurrences",
            "scheduled_reminders",
            "board",
            "suggestions",
            "attachments",
            "lists",
        ] {
            assert!(page.get(key).is_none(), "continued page repeated {key}");
        }
    }
    let c = Caller::Member {
        household_id: h,
        member_id: m,
        user_id: Uuid::nil(),
        role: "super_admin".into(),
        device_id: Uuid::now_v7(),
        session_id: Uuid::nil(),
        request_id: Uuid::nil(),
        client_addr: None,
    };
    let mut conn = pool.acquire().await.unwrap();
    for (f, t) in windows {
        let rows = agenda_rows(&mut conn, &c, m, f, t, None, i32::MAX as usize, Some(id))
            .await
            .unwrap();
        let key = serde_json::to_value(f)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            actual[&key],
            rows.iter().map(|r| digest(&r.1)).collect::<Vec<_>>(),
            "continuation union must equal unbounded agenda"
        );
    }
    let expected = crate::routes::collaboration::comment_rows(&mut conn, id, None, 1000, None)
        .await
        .unwrap();
    assert_eq!(
        comments,
        expected
            .iter()
            .map(|r| digest(&serde_json::to_value(r).unwrap()))
            .collect::<Vec<_>>()
    );
    println!(
        "ITEM_SYNC continuation_pages={pages} comments={} windows=7 union_matches_unbounded=true",
        comments.len()
    );
    s.stop().await;
}
