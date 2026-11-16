use super::*;
use crate::testing::{TestOptions, TestServer};
use axum::body::to_bytes;
use sqlx::PgPool;
use std::time::Instant;

fn caller(h: porchlight_db::testing::Household, member: Uuid) -> Caller {
    Caller::Member {
        member_id: member,
        user_id: Uuid::nil(),
        role: if member == h.super_admin {
            "super_admin"
        } else if member == h.dependent {
            "dependent"
        } else {
            "user"
        }
        .into(),
        device_id: Uuid::now_v7(),
        session_id: Uuid::nil(),
        household_id: h.id,
        client_addr: None,
        request_id: Uuid::now_v7(),
    }
}
async fn fixture(pool: &PgPool, count: i32) -> (TestServer, porchlight_db::testing::Household) {
    let s = TestServer::start_with(
        pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let h = porchlight_db::testing::seed_household(pool).await.unwrap();
    sqlx::query("insert into porchlight.items(household_id,owner_id,kind,title,notes,time_mode,starts_at,ends_at,tzid,visibility) select $1,$2,'appointment','Snapshot fixture ' || n,repeat('Fixture notes ',20),'fixed','2026-10-06T09:00Z','2026-10-06T10:00Z','UTC',case n%3 when 0 then 'private' when 1 then 'selected' else 'household' end from generate_series(1,$3) n")
        .bind(h.id).bind(h.super_admin).bind(count).execute(pool).await.unwrap();
    sqlx::query("insert into porchlight.item_viewers(household_id,item_id,member_id) select household_id,id,$2 from porchlight.items where household_id=$1 and visibility='selected'")
        .bind(h.id).bind(h.user).execute(pool).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at,override_local_start,override_local_end) select $1,i.id,n,'2026-10-06T09:00Z'::timestamptz+n*interval '1 day','2026-10-06T10:00Z'::timestamptz+n*interval '1 day',case when n=1 then '2026-10-07T11:00'::timestamp end,case when n=1 then '2026-10-07T12:00'::timestamp end from porchlight.items i cross join generate_series(0,9) n where i.household_id=$1")
        .bind(h.id).execute(pool).await.unwrap();
    sqlx::query("insert into porchlight.comments(household_id,item_id,author_id,body) select $1,id,$2,repeat('Fixture comment ',30) from porchlight.items where household_id=$1")
        .bind(h.id).bind(h.super_admin).execute(pool).await.unwrap();
    // Populate all snapshot extension shapes (no filesystem bytes are needed for metadata).
    for sql in [
        "insert into porchlight.blobs(household_id,sha256,size,content_type) values($1,repeat('a',64),12,'text/plain')",
        "insert into porchlight.attachments(household_id,item_id,blob_sha256,original_name,uploader_id) select $1,id,repeat('a',64),'fixture.txt',owner_id from porchlight.items where household_id=$1 limit 1",
        "insert into porchlight.item_reminders(household_id,item_id,offset_minutes) select $1,id,10 from porchlight.items where household_id=$1 limit 1",
        "insert into porchlight.scheduled_reminders(household_id,item_id,member_id,occurrence_id,reminder_id,fire_at) select $1,r.item_id,i.owner_id,o.id,r.id,'2026-10-06T08:50Z' from porchlight.item_reminders r join porchlight.items i on i.id=r.item_id join porchlight.item_occurrences o on o.item_id=i.id and o.seq=0 where i.household_id=$1",
        "insert into porchlight.lists(household_id,owner_id,name) select $1,owner_id,'Fixture list' from porchlight.items where household_id=$1 limit 1",
        "insert into porchlight.list_entries(household_id,list_id,label) select $1,id,'Fixture entry' from porchlight.lists where household_id=$1",
        "insert into porchlight.availability_profiles(household_id,member_id) select $1,id from porchlight.household_members where household_id=$1",
        "insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) select $1,id,0,'09:00','17:00' from porchlight.household_members where household_id=$1",
        "insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) select $1,i.owner_id,i.id,o.id,'2026-10-06T12:00Z','2026-10-06T13:00Z',uuidv7() from porchlight.items i join porchlight.item_occurrences o on o.item_id=i.id and o.seq=0 where i.household_id=$1 limit 1",
        "insert into porchlight.notifications(household_id,recipient_member_id,kind,category,item_id,entity_type,entity_id,window_start,release_at,released_at) select $1,owner_id,'item_assigned','assignments',id,'item',id,now(),now(),now() from porchlight.items where household_id=$1 limit 1",
        "insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,processed_at) select $1,recipient_member_id,'assigned',item_id,'item',item_id,now() from porchlight.notifications where household_id=$1",
        "insert into porchlight.notification_sources(notification_id,event_id) select n.id,e.id from porchlight.notifications n join porchlight.notification_events e on e.item_id=n.item_id and e.household_id=n.household_id where n.household_id=$1",
        "insert into porchlight.ics_sources(household_id,owner_member_id,kind,import_key,name,url,enabled) select $1,owner_id,'feed','fixture','Fixture feed','https://calendar.example.test/private-fixture',false from porchlight.items where household_id=$1 limit 1",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(h.id)
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("insert into porchlight.items(household_id,owner_id,kind,title,time_mode,deleted_at) values($1,$2,'todo','Deleted fixture','fixed',now())")
        .bind(h.id).bind(h.super_admin).execute(pool).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let pending: i64 = sqlx::query_scalar(
                "select count(*) from porchlight.outbox where stream_seq is null",
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if pending == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    (s, h)
}
async fn body(response: axum::response::Response) -> axum::body::Bytes {
    to_bytes(response.into_body(), usize::MAX).await.unwrap()
}
fn compare(path: &str, old: &Value, new: &Value) {
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (k, v) in a {
                compare(
                    &format!("{path}.{k}"),
                    v,
                    b.get(k).expect("parity object key missing"),
                );
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (n, (a, b)) in a.iter().zip(b).enumerate() {
                compare(&format!("{path}[{n}]"), a, b);
            }
        }
        _ => assert_eq!(old, new, "{path}"),
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_parity(pool: PgPool) {
    let (s, h) = fixture(&pool, 6).await;
    for member in [h.super_admin, h.user, h.dependent] {
        let old = body(
            legacy_snapshot(State(s.state.clone()), caller(h, member))
                .await
                .unwrap(),
        )
        .await;
        let new = body(
            snapshot(State(s.state.clone()), caller(h, member))
                .await
                .unwrap(),
        )
        .await;
        document_parity::unique(&old).unwrap();
        document_parity::unique(&new).unwrap();
        let old: Value = serde_json::from_slice(&old).unwrap();
        let new: Value = serde_json::from_slice(&new).unwrap();
        compare("snapshot", &old, &new);
        if member == h.super_admin {
            for key in [
                "items",
                "occurrences",
                "comments",
                "attachments",
                "lists",
                "scheduled_reminders",
                "notifications",
                "availability",
                "suggestions",
            ] {
                assert!(
                    !new[key].as_array().unwrap().is_empty(),
                    "fixture missing {key}"
                );
            }
        }
    }
    s.stop().await;
}

// Only this selected test runs in the allocator-instrumented process. Normal lib
// tests still exercise its large fixture and parity without global peak assertions.
#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_cost(pool: PgPool) {
    let (s, h) = fixture(&pool, 1000).await;
    for legacy in [true, false] {
        #[cfg(porchlight_snapshot_probe)]
        let base = super::alloc_probe::start();
        let start = Instant::now();
        let response = if legacy {
            legacy_snapshot(State(s.state.clone()), caller(h, h.super_admin)).await
        } else {
            snapshot(State(s.state.clone()), caller(h, h.super_admin)).await
        }
        .unwrap();
        let bytes = body(response).await;
        let elapsed = start.elapsed();
        #[cfg(porchlight_snapshot_probe)]
        let peak = super::alloc_probe::peak(base);
        #[cfg(not(porchlight_snapshot_probe))]
        let peak = 0usize;
        let gzip_start = Instant::now();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(1));
        std::io::Write::write_all(&mut encoder, &bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let gzip_elapsed = gzip_start.elapsed();
        println!(
            "SNAPSHOT legacy={legacy} bytes={} gzip={} peak={peak} single_ms={} gzip_ms={}",
            bytes.len(),
            compressed.len(),
            elapsed.as_millis(),
            gzip_elapsed.as_millis()
        );
        assert!(
            legacy || peak * 2 <= bytes.len() * 5,
            "snapshot heap {peak} exceeds 2.5x response {}",
            bytes.len()
        );
        assert!(
            legacy || peak <= crate::state::SNAPSHOT_ESTIMATED_HEAP,
            "reference workload heap {peak} exceeds admission estimate"
        );
        drop(bytes);
        drop(compressed);
        let start = Instant::now();
        let requests = (0..4).map(|n| {
            let s = &s;
            async move {
                let c = caller(h, if n == 3 { h.user } else { h.super_admin });
                let response = if legacy {
                    legacy_snapshot(State(s.state.clone()), c).await
                } else {
                    snapshot(State(s.state.clone()), c).await
                }
                .unwrap();
                body(response).await.len()
            }
        });
        let sizes = futures_util::future::join_all(requests).await;
        println!(
            "SNAPSHOT legacy={legacy} concurrent4_ms={} sizes={sizes:?}",
            start.elapsed().as_millis()
        );
    }
    s.stop().await;
}

#[path = "snapshot_document_parity.rs"]
mod document_parity;
#[path = "item_sync_probe.rs"]
mod item_sync_probe;
#[path = "snapshot_router_probe.rs"]
mod router_probe;
