//! Source identity remains stable even when a producer changes recurrence representation.
use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn representation_changes_do_not_overwrite_master_or_leave_duplicate_busy_blocks(
    pool: PgPool,
) {
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
    let retained = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:series\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Master\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:series\r\nRECURRENCE-ID:20261013T090000Z\r\nDTSTART:20261013T110000Z\r\nDTEND:20261013T120000Z\r\nSUMMARY:Moved\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let expanded = retained.replace("FREQ=DAILY;COUNT=3", "FREQ=DAILY;COUNT=3;BYHOUR=9");
    let mut tx = porchlight_db::tx::begin_as(&s.state.pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    let id: uuid::Uuid = sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url) values($1,$2,$2,'feed','transition','transition','https://example.invalid/calendar') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let caller = porchlight_api::extract::Caller::Agent {
        token_id: uuid::Uuid::nil(),
        member_id: s.init.admin_member_id,
        label: "test".into(),
        scopes: vec!["admin".into()],
        household_id: s.init.household_id,
        request_id: uuid::Uuid::now_v7(),
    };
    for (index, content) in [retained, expanded.as_str(), retained].iter().enumerate() {
        let mut tx =
            porchlight_db::tx::begin_as(&s.state.pool, &porchlight_db::tx::Actor::system())
                .await
                .unwrap();
        let source = porchlight_api::calendar::load_source(&mut tx, s.init.household_id, id, true)
            .await
            .unwrap();
        porchlight_api::items::ics_import::apply_calendar(
            &mut tx,
            &caller,
            &source,
            content,
            true,
            "2026-10-06T12:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let busy: i64 = sqlx::query_scalar("select count(*) from porchlight.member_agenda a join porchlight.items i on i.id=a.item_id where i.ics_source_id=$1 and i.deleted_at is null and a.busy is not null")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(busy, 3, "representation step {index}");
    }
    // Even super admins cannot mutate feed items through the app; agents retain edit.
    let id: uuid::Uuid = sqlx::query_scalar(
        "select id from porchlight.items where ics_source_id=$1 and deleted_at is null limit 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let path = format!("/v1/items/{id}");
    let response = c
        .request(s.lan, d.request("GET", &path, json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    let occurrence: (uuid::Uuid,i32)=sqlx::query_as("select id,version from porchlight.item_occurrences where item_id=$1 and status='todo' limit 1").bind(id).fetch_one(&pool).await.unwrap();
    for (method, route, body, version) in [
        (
            "PATCH",
            path.clone(),
            json!({"title":"no"}),
            response.json()["version"].as_i64().unwrap() as i32,
        ),
        (
            "DELETE",
            path.clone(),
            json!({}),
            response.json()["version"].as_i64().unwrap() as i32,
        ),
        (
            "PUT",
            format!("{path}/assignees"),
            json!({"assignees":[]}),
            response.json()["version"].as_i64().unwrap() as i32,
        ),
        (
            "POST",
            format!("/v1/occurrences/{}/complete", occurrence.0),
            json!({}),
            occurrence.1,
        ),
    ] {
        let mut request = d.request(
            method,
            &route,
            body,
            Some(&format!("denied-{method}-{version}")),
        );
        request
            .headers_mut()
            .insert("if-match", version.to_string().parse().unwrap());
        assert_eq!(c.request(s.lan, request).await.unwrap().status, 403);
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn removed_source_exception_does_not_keep_ownership_of_human_skip(pool: PgPool) {
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
    let cal = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:series\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEXDATE:20261013T090000Z\r\nSUMMARY:Master\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let reset = cal.replace("EXDATE:20261013T090000Z\r\n", "");
    for (key, content) in [("initial", cal), ("reset", reset.as_str())] {
        let response = c
            .request(
                s.lan,
                d.request(
                    "POST",
                    "/v1/imports/ics",
                    json!({"content":content,"member_id":s.init.admin_member_id}),
                    Some(key),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 200);
    }
    // A subsequent raw agent human skip is no longer source-managed.
    let mut tx = porchlight_db::tx::begin_as(&s.state.pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    sqlx::query("update porchlight.item_occurrences set status='skipped' where seq=1")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let response = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/imports/ics",
                json!({"content":reset,"member_id":s.init.admin_member_id}),
                Some("repeat"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    let status: String =
        sqlx::query_scalar("select status from porchlight.item_occurrences where seq=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "skipped");
    s.stop().await;
}
