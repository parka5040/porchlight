use axum::body::to_bytes;
use chrono::{DateTime, Utc};
use porchlight_api::{
    notifier, routes,
    server::{ConnInfo, Transport},
    testing::{TestOptions, TestServer, with_notification_source_probe},
};
use porchlight_core::auth::session::Platform;
use porchlight_db::testing::{Role, insert_appointment, pool_as, seed_household};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;

fn at(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
async fn body(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), 200);
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn digest_merges_decode_each_event_once_across_passes_and_restart(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let hidden = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end='07:00' where member_id=$1")
        .bind(h.super_admin).execute(&pool).await.unwrap();
    let night = at("2026-10-06T03:00:00Z");
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,created_at) select $1,$2,case when n%2=0 then 'comment' else 'assigned' end,case when n%2=0 then $5::uuid else $3 end,'item',$3,$4 from generate_series(1,501) n")
        .bind(h.id).bind(h.super_admin).bind(item).bind(night).bind(hidden).execute(&pool).await.unwrap();
    let mut counts = Vec::new();
    for expected in [500, 1] {
        // A fresh pool exercises persistent aggregation, not a process-local cache.
        let app = pool_as(&pool, Role::App, None).await.unwrap();
        let (result, work) = with_notification_source_probe(notifier::drain(&app, night)).await;
        assert_eq!(result.unwrap(), expected);
        counts.extend(work);
        app.close().await;
    }
    assert!(
        !counts.is_empty(),
        "source query instrumentation must be active"
    );
    assert_eq!(
        counts.iter().sum::<usize>(),
        501,
        "digest merges must decode each new source once, never reload history"
    );
    assert_eq!(counts.len(), 2, "one source discovery per recipient pass");
    let (count, summary): (i32, Value) = sqlx::query_as(
        "select count,summary from porchlight.notifications where category='digest'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 501);
    assert_eq!(summary["text"], "Morning digest: 501 updates");
    assert_eq!(summary["local_alarm"], false);
    let morning = at("2026-10-06T12:00:00Z");
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    notifier::drain(&app, morning).await.unwrap();
    sqlx::query("update porchlight.notifications set read_at=$1")
        .bind(morning)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,created_at) values($1,$2,'comment',$3,'item',$3,$4)")
        .bind(h.id).bind(h.super_admin).bind(item).bind(night).execute(&pool).await.unwrap();
    let (result, late) = with_notification_source_probe(notifier::drain(&app, morning)).await;
    result.unwrap();
    assert_eq!(late, vec![1]);
    let rows: Vec<(i32, Value, Option<DateTime<Utc>>)> =
        sqlx::query_as("select count,summary,read_at from porchlight.notifications")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 502);
    assert_eq!(rows[0].1["text"], "Morning digest: 502 updates");
    assert_eq!(rows[0].2, Some(morning));
    let id: uuid::Uuid = sqlx::query_scalar("select id from porchlight.notifications")
        .fetch_one(&pool)
        .await
        .unwrap();
    // Current ACLs, not the incremental stored count, determine the exact digest.
    sqlx::query("update porchlight.items set owner_id=$2,visibility='private' where id=$1")
        .bind(hidden)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let mut c = app.acquire().await.unwrap();
    let visible = notifier::visible_source_summaries(&mut c, &[id], h.super_admin, h.id)
        .await
        .unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].count, 252);
    assert_eq!(visible[0].summary(true).text, "Morning digest: 252 updates");
    sqlx::query("update porchlight.items set owner_id=$2,visibility='private' where id=$1")
        .bind(item)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        notifier::visible_source_summaries(&mut c, &[id], h.super_admin, h.id)
            .await
            .unwrap()
            .is_empty()
    );
    drop(c);
    app.close().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn inbox_and_snapshot_batch_source_details_without_truncating_acl_counts(pool: PgPool) {
    let t = at("2026-10-05T12:00:00Z");
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(move || t),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let router = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let hidden: uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'Private source','todo') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,entity_type,entity_id,created_at) select $1,$2,'comment','member',$2,$3::timestamptz+n*interval '31 seconds' from generate_series(0,204) n")
        .bind(s.init.household_id).bind(s.init.admin_member_id).bind(t).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,created_at) select $1,$2,case when n%2=0 then 'assigned' else 'comment' end,case when n%2=0 then $4::uuid end,'member',$2,$3 from generate_series(1,450) n")
        .bind(s.init.household_id).bind(s.init.admin_member_id).bind(t).bind(hidden).execute(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let now = t + chrono::Duration::hours(2);
    for expected in [500, 155] {
        assert_eq!(notifier::drain(&app, now).await.unwrap(), expected);
    }
    sqlx::query("update porchlight.items set deleted_at=$2 where id=$1")
        .bind(hidden)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    for (url, field, expected_rows) in [
        ("/v1/notifications?limit=200", "items", 200),
        ("/v1/snapshot", "notifications", 205),
    ] {
        let (response, counts) = with_notification_source_probe(router.clone().oneshot(d.request(
            "GET",
            url,
            json!({}),
            None,
        )))
        .await;
        let value = body(response.unwrap()).await;
        assert_eq!(value[field].as_array().unwrap().len(), expected_rows);
        assert_eq!(
            counts.len(),
            2,
            "detail reads must batch at most 200 notification IDs, not query each row"
        );
        assert!(
            counts.iter().all(|n| *n <= 200),
            "source result rows must be bounded independently of digest size"
        );
        if field == "notifications" {
            assert_eq!(counts.iter().sum::<usize>(), 205);
            let large = value[field]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["count"] == 226)
                .unwrap();
            assert_eq!(large["category"], "activity");
            assert_eq!(large["summary"]["category"], "comments");
            assert_eq!(
                large["summary"]["text"],
                "Porchlight: 226 household updates"
            );
            assert!(large["item_id"].is_null());
        }
    }
    app.close().await;
    s.stop().await;
}
