use porchlight_api::testing::{TestClient, TestOptions, TestServer, trust};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;

const CAL: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:school\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEXDATE:20261013T090000Z\r\nSUMMARY:School\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:other\r\nDTSTART:20261015T090000Z\r\nDTEND:20261015T100000Z\r\nSUMMARY:Other\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
async fn snapshot(pool: &PgPool) -> Value {
    sqlx::query_scalar("select jsonb_build_object('items',(select jsonb_agg(jsonb_build_array(id,version) order by id) from porchlight.items),'occurrences',(select jsonb_agg(jsonb_build_array(id,version,skipped_at) order by id) from porchlight.item_occurrences),'revisions',(select count(*) from porchlight.item_revisions),'outbox',(select count(*) from porchlight.outbox))").fetch_one(pool).await.unwrap()
}
async fn uid_snapshot(pool: &PgPool, uid: &str) -> Value {
    sqlx::query_scalar("select jsonb_build_object('items',(select jsonb_agg(to_jsonb(i) order by i.id) from porchlight.items i where i.id in (select item_id from porchlight.ics_uid_map where uid=$1)),'occurrences',(select jsonb_agg(to_jsonb(o) order by o.id) from porchlight.item_occurrences o where o.item_id in (select item_id from porchlight.ics_uid_map where uid=$1)))").bind(uid).fetch_one(pool).await.unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn unchanged_import_writes_nothing_and_change_updates_only_one(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let body = json!({"content":CAL,"member_id":s.init.admin_member_id});
    let first = c
        .request(
            s.lan,
            d.request("POST", "/v1/imports/ics", body.clone(), Some("first")),
        )
        .await
        .unwrap();
    assert_eq!(first.status, 200);
    let before = snapshot(&pool).await;
    let second = c
        .request(
            s.lan,
            d.request("POST", "/v1/imports/ics", body.clone(), Some("second")),
        )
        .await
        .unwrap();
    assert_eq!(second.status, 200);
    assert_eq!(
        snapshot(&pool).await,
        before,
        "identical import must not write domain rows"
    );
    assert_eq!(second.json()["unchanged"], 2);
    assert_eq!(second.json()["updated"], 0);
    assert_eq!(second.json()["skipped"], 0);
    let school_before = uid_snapshot(&pool, "school").await;
    let mut visibility_only = body.clone();
    visibility_only["visibility"] = json!("household");
    let fourth = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/imports/ics",
                visibility_only,
                Some("visibility-only"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(fourth.status, 200);
    assert_eq!(fourth.json()["unchanged"], 2);
    // Source config changes, but existing editable import items keep their visibility.
    assert_eq!(school_before, uid_snapshot(&pool, "school").await);
    let mut changed = body;
    changed["content"] = CAL.replace("SUMMARY:Other", "SUMMARY:Changed").into();
    let third = c
        .request(
            s.lan,
            d.request("POST", "/v1/imports/ics", changed, Some("third")),
        )
        .await
        .unwrap();
    assert_eq!(third.status, 200);
    assert_eq!(third.json()["updated"], 1);
    assert_eq!(third.json()["unchanged"], 1);
    assert_eq!(
        school_before,
        uid_snapshot(&pool, "school").await,
        "every item/occurrence of the other UID, including EXDATE, is unchanged"
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn import_for_another_member_is_owned_private_and_visible_to_target(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let (_, target) = trust::user_named(&pool, &s, "target", "Target").await;
    let (_, other) = trust::user_named(&pool, &s, "other", "Other").await;
    let admin = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let a = TestClient::new(&s.ca_pem, Some(&admin));
    let response = a
        .request(
            s.lan,
            admin.request(
                "POST",
                "/v1/imports/ics",
                json!({"content":CAL,"member_id":target}),
                Some("target"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    let item = response.json()["item_ids"][0].as_str().unwrap().to_owned();
    let row: (uuid::Uuid, String) =
        sqlx::query_as("select owner_id,visibility from porchlight.items where id=$1")
            .bind(item.parse::<uuid::Uuid>().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, (target, "private".into()));
    for (member, expected) in [(target, 200), (other, 404), (s.init.admin_member_id, 404)] {
        let d = s.session_device(member, Platform::Desktop).await;
        let c = TestClient::new(&s.ca_pem, Some(&d));
        assert_eq!(
            c.request(
                s.lan,
                d.request("GET", &format!("/v1/items/{item}"), json!({}), None)
            )
            .await
            .unwrap()
            .status,
            expected
        );
    }
    let busy: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.member_agenda where member_id=$1 and item_id=$2",
    )
    .bind(target)
    .bind(item.parse::<uuid::Uuid>().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(busy > 0);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn unknown_body_members_are_uniform_field_validation(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    for (path, field, body) in [
        (
            "/v1/imports/ics",
            "member_id",
            json!({"content":CAL,"member_id":uuid::Uuid::now_v7()}),
        ),
        (
            "/v1/feeds",
            "target_member_id",
            json!({"name":"bad","url":"https://example.invalid/feed","target_member_id":uuid::Uuid::now_v7()}),
        ),
    ] {
        let response = c
            .request(s.lan, d.request("POST", path, body, Some(field)))
            .await
            .unwrap();
        assert_eq!(response.status, 422);
        assert!(
            response.json()["type"]
                .as_str()
                .unwrap()
                .ends_with("validation-failed")
        );
        assert!(
            response.json().to_string().contains(field),
            "field pointer missing"
        );
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn escaped_json_near_raw_calendar_limit_is_accepted(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let content = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-ENVELOPE:{}\r\n{}",
        "\\".repeat(5 * 1024 * 1024 - CAL.len() - 100),
        CAL.split_once("VERSION:2.0\r\n").unwrap().1
    );
    let response = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/imports/ics",
                json!({"content":content,"member_id":s.init.admin_member_id}),
                Some("large-json"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn invisible_mapped_item_does_not_block_other_scope_events(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let first = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/imports/ics",
                json!({"content":CAL,"member_id":s.init.admin_member_id}),
                Some("scope-first"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(first.status, 200);
    let hidden: uuid::Uuid =
        sqlx::query_scalar("select id from porchlight.items where title='School'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let (_, target) = trust::user_named(&pool, &s, "scopehidden", "Hidden").await;
    let mut tx = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    sqlx::query("update porchlight.items set owner_id=$2 where id=$1")
        .bind(hidden)
        .bind(target)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let response=c.request(s.lan,d.request("POST","/v1/imports/ics",json!({"content":CAL.replace("SUMMARY:Other","SUMMARY:Updated"),"member_id":s.init.admin_member_id}),Some("scope-second"))).await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["updated"], 1);
    assert!(
        response.json()["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "InvisibleItem")
    );
    s.stop().await;
}
