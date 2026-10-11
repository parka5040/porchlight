use porchlight_api::testing::{HouseholdFixture, TestClient, TestOptions, trust};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn fixture(pool: &PgPool) -> HouseholdFixture {
    HouseholdFixture::build_with(
        pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await
}

#[sqlx::test(migrations = "../db/migrations")]
async fn member_and_admin_target_never_export_pre_share_history(pool: PgPool) {
    let f = fixture(&pool).await;
    let h = f.server.init.household_id;
    let owner = f.admin.member_id;
    let reader = &f.users[0];
    let item: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,kind,title,notes,visibility) values($1,$2,'todo','Clean title','private-era item sentinel','private') returning id").bind(h).bind(owner).fetch_one(&pool).await.unwrap();
    sqlx::query("update porchlight.items set notes='',visibility='household' where id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    let list: Uuid = sqlx::query_scalar("insert into porchlight.lists(household_id,owner_id,name,visibility) values($1,$2,'private-era list sentinel','private') returning id").bind(h).bind(owner).fetch_one(&pool).await.unwrap();
    let archived: Uuid = sqlx::query_scalar("insert into porchlight.list_entries(household_id,list_id,label) values($1,$2,'archived-private sentinel') returning id").bind(h).bind(list).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.list_entry_checks(household_id,entry_id,checked_by) values($1,$2,$3)").bind(h).bind(archived).bind(owner).execute(&pool).await.unwrap();
    sqlx::query("update porchlight.list_entries set archived_at='2026-10-01' where id=$1")
        .bind(archived)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.lists set name='Clean list',visibility='household' where id=$1")
        .bind(list)
        .execute(&pool)
        .await
        .unwrap();
    let entry: Uuid = sqlx::query_scalar("insert into porchlight.list_entries(household_id,list_id,label) values($1,$2,'old-entry sentinel') returning id").bind(h).bind(list).fetch_one(&pool).await.unwrap();
    sqlx::query("update porchlight.list_entries set label='Clean entry' where id=$1")
        .bind(entry)
        .execute(&pool)
        .await
        .unwrap();
    let own = TestClient::new(&f.server.ca_pem, Some(&reader.device))
        .request(
            f.server.lan,
            reader
                .device
                .request("GET", "/v1/export/member", json!({}), None),
        )
        .await
        .unwrap();
    let target = trust::admin(
        &f.server,
        "GET",
        &format!("/v1/export/member?member_id={}", reader.member_id),
        json!({}),
    )
    .await;
    for r in [own, target] {
        assert_eq!(r.status, 200);
        let data = r.json();
        assert!(data.to_string().contains("Clean title"));
        assert!(data.to_string().contains("Clean list"));
        assert!(
            !data.to_string().contains(&archived.to_string()),
            "archived entry/check leaked"
        );
        assert!(data.to_string().contains("Clean entry"));
        assert!(
            !data.to_string().contains("sentinel"),
            "private history leaked"
        );
        for table in [
            "item_revisions",
            "task_events",
            "list_revisions",
            "list_entry_revisions",
            "item_derivation",
        ] {
            assert!(data["tables"].get(table).is_none(), "history table {table}");
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn household_headers_precede_table_reads_and_disconnect_releases_lease(pool: PgPool) {
    use hyper_util::rt::TokioIo;
    use tokio::net::UnixStream;
    let f = fixture(&pool).await;
    // A sizeable table plus an intentional blocked read: returning headers must not
    // depend on materializing this table. No timing-dependent overlap assumption.
    sqlx::query("insert into porchlight.items(household_id,owner_id,kind,title,notes) select $1,$2,'todo','Large export '||g,repeat(md5(g::text),256) from generate_series(1,2000) g").bind(f.server.init.household_id).bind(f.admin.member_id).execute(&pool).await.unwrap();
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("lock table porchlight.items in access exclusive mode")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let io = UnixStream::connect(&f.server.agent_socket).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .unwrap();
    let task = tokio::spawn(connection);
    let mut req = trust::req(
        "GET",
        "/v1/admin/export/household",
        json!({}),
        "headers-test",
    );
    req.headers_mut().insert(
        "authorization",
        format!("Bearer {}", f.server.bootstrap_token())
            .parse()
            .unwrap(),
    );
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(2), sender.send_request(req))
            .await
            .expect("headers waited for table work")
            .unwrap();
    assert_eq!(response.status(), 200);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let count: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.blob_leases where holder='household-export'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if count > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "export never acquired lease"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    drop(response);
    drop(sender);
    task.abort();
    loop {
        let count: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.blob_leases where holder='household-export'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if count == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "disconnect retained lease during blocked preparation"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    blocker.rollback().await.unwrap();
    // Complete the same large fixture and inspect table-local JSONL output.
    let r = trust::admin(&f.server, "GET", "/v1/admin/export/household", json!({})).await;
    assert_eq!(r.status, 200);
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(r.body.as_ref()));
    let mut items = 0;
    for e in tar.entries().unwrap() {
        let e = e.unwrap();
        if e.path().unwrap().as_ref() == std::path::Path::new("tables/items.jsonl") {
            use std::io::BufRead;
            for line in std::io::BufReader::new(e).lines() {
                let row: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
                assert!(row["title"].as_str().unwrap().starts_with("Large export "));
                items += 1;
            }
        }
    }
    assert_eq!(items, 2000);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ics_uses_effective_overrides_and_time_mode(pool: PgPool) {
    let f = fixture(&pool).await;
    let cases = [
        json!({"title":"Moved fixed","kind":"appointment","time_mode":"fixed","starts_at":"2026-10-12T09:00:00Z","ends_at":"2026-10-12T10:00:00Z","tzid":"UTC"}),
        json!({"title":"Moved floating","kind":"appointment","time_mode":"floating","local_start":"2026-10-12T09:00:00","local_end":"2026-10-12T10:00:00"}),
        json!({"title":"Moved all-day","kind":"appointment","time_mode":"floating","all_day":true,"local_start":"2026-10-12T00:00:00","local_end":"2026-10-13T00:00:00"}),
        json!({"title":"Moved across DST","kind":"appointment","time_mode":"fixed","starts_at":"2026-03-01T14:00:00Z","ends_at":"2026-03-01T15:00:00Z","tzid":"America/New_York"}),
        json!({"title":"Fold","kind":"appointment","time_mode":"fixed","starts_at":"2026-11-01T06:30:00Z","ends_at":"2026-11-01T07:30:00Z","tzid":"America/New_York"}),
    ];
    for (n, case) in cases.into_iter().enumerate() {
        let r = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device))
            .request(
                f.server.lan,
                f.users[0].device.request(
                    "POST",
                    "/v1/items",
                    case,
                    Some(&Uuid::now_v7().to_string()),
                ),
            )
            .await
            .unwrap();
        assert_eq!(r.status, 201, "{}", r.json());
        let id: Uuid = r.json()["id"].as_str().unwrap().parse().unwrap();
        if n < 4 {
            let (start, end) = if n == 3 {
                ("2026-03-09 09:00:00", "2026-03-09 10:00:00")
            } else if n == 2 {
                ("2026-10-15 00:00:00", "2026-10-16 00:00:00")
            } else {
                ("2026-10-15 13:00:00", "2026-10-15 14:00:00")
            };
            sqlx::query("update porchlight.item_occurrences set override_local_start=$2::text::timestamp,override_local_end=$3::text::timestamp where item_id=$1").bind(id).bind(start).bind(end).execute(&pool).await.unwrap();
            if n == 1 {
                // A stale/generated UTC value must not change floating semantics.
                sqlx::query("update porchlight.item_occurrences set starts_at='2026-10-12T09:00Z',ends_at='2026-10-12T10:00Z' where item_id=$1").bind(id).execute(&pool).await.unwrap();
            }
        }
    }
    let r = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device))
        .request(
            f.server.lan,
            f.users[0]
                .device
                .request("GET", "/v1/export/member", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    let data = r.json();
    let ics = data["appointments_ics"].as_str().unwrap();
    assert!(ics.contains("DTSTART:20261015T130000Z"), "{ics}");
    assert!(ics.contains("DTSTART:20261015T130000\r\n"), "{ics}");
    assert!(ics.contains("DTSTART;VALUE=DATE:20261015"), "{ics}");
    assert!(ics.contains("DTSTART:20261101T063000Z"), "{ics}");
    let parsed = porchlight_core::ics::parse(
        ics,
        porchlight_core::ics::ParseOptions {
            fallback_tz: chrono_tz::UTC,
            horizon_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            horizon_end: "2028-01-01T00:00:00Z".parse().unwrap(),
        },
    );
    assert_eq!(parsed.events.len(), 5);
    let dst = parsed
        .events
        .iter()
        .find(|e| e.summary == "Moved across DST")
        .unwrap();
    assert_eq!(
        dst.start,
        porchlight_core::ics::IcsTime::Fixed("2026-03-09T13:00:00Z".parse().unwrap())
    );
    assert_eq!(
        dst.end,
        porchlight_core::ics::IcsTime::Fixed("2026-03-09T14:00:00Z".parse().unwrap())
    );
}

/// The flake behind `household_headers_precede_table_reads_and_disconnect_releases_lease`: a
/// disconnect while the export's BEGIN was in flight stranded that transaction on a pooled
/// connection, so a later export's lease and `SET TRANSACTION` ran inside it and the body was
/// truncated. Disconnecting the moment the lease is visible lands on that BEGIN in most cycles.
/// End-to-end and probabilistic in the red direction (about half the cycles hit the window);
/// the deterministic guard is porchlight-db's `tx_begin_cancel` test.
#[sqlx::test(migrations = "../db/migrations")]
async fn disconnect_during_export_begin_never_poisons_the_pool(pool: PgPool) {
    use hyper_util::rt::TokioIo;
    use tokio::net::UnixStream;
    let f = fixture(&pool).await;
    let leases = "select count(*) from porchlight.blob_leases where holder='household-export'";
    for cycle in 0..40 {
        let io = UnixStream::connect(&f.server.agent_socket).await.unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
            .await
            .unwrap();
        let task = tokio::spawn(connection);
        let mut req = trust::req("GET", "/v1/admin/export/household", json!({}), "cancel");
        req.headers_mut().insert(
            "authorization",
            format!("Bearer {}", f.server.bootstrap_token())
                .parse()
                .unwrap(),
        );
        let response = sender.send_request(req).await.unwrap();
        assert_eq!(response.status(), 200);
        // Liveness bounds only; no overlap assumption.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while sqlx::query_scalar::<_, i64>(leases)
            .fetch_one(&pool)
            .await
            .unwrap()
            == 0
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "cycle {cycle}: no lease"
            );
        }
        drop(response);
        drop(sender);
        task.abort();
        while sqlx::query_scalar::<_, i64>(leases)
            .fetch_one(&pool)
            .await
            .unwrap()
            > 0
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "cycle {cycle}: lease release never committed"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // Lease cleanup can precede the producer's final teardown and admission release.
        let r = loop {
            let r = trust::admin(&f.server, "GET", "/v1/admin/export/household", json!({})).await;
            if r.status != 429 {
                break r;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "cycle {cycle}: export permit leaked"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        assert_eq!(r.status, 200);
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(r.body.as_ref()));
        let last = tar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().into_owned())
            .last();
        assert_eq!(
            last.as_deref(),
            Some(std::path::Path::new("complete.json")),
            "cycle {cycle}: export after a disconnect was incomplete"
        );
    }
}
