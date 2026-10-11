use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
fn options() -> TestOptions {
    TestOptions {
        db_pool_size: 1,
        background_derivation: false,
        item_clock: Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
        ..Default::default()
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn availability_edf_accept_replay_stale_dismiss_and_raw_convergence(pool: PgPool) {
    let s = TestServer::start_with(&pool, options()).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let path = format!("/v1/members/{}/availability", s.init.admin_member_id);
    let get = client
        .request(s.lan, d.request("GET", &path, json!({}), None))
        .await
        .unwrap();
    assert_eq!(get.status, 200);
    assert_eq!(get.json()["version"], 0);
    assert_eq!(get.json()["windows"], json!([]));
    let mut req = d.request(
        "PUT",
        &path,
        json!({"windows":[{"weekday":0,"start":"09:00:00","end":"17:00:00"}]}),
        Some("windows"),
    );
    req.headers_mut().insert("if-match", "0".parse().unwrap());
    let put = client.request(s.lan, req).await.unwrap();
    assert_eq!(put.status, 200, "{}", put.json());
    let mut stale = d.request("PUT", &path, json!({"windows":[]}), Some("stale"));
    stale.headers_mut().insert("if-match", "0".parse().unwrap());
    let stale = client.request(s.lan, stale).await.unwrap();
    assert_eq!(stale.status, 409);
    assert_eq!(stale.json()["current"], put.json());
    let mut item_ids = Vec::new();
    for (title, est) in [("First", 60), ("Second", 30)] {
        let r=client.request(s.lan,d.request("POST","/v1/items",json!({"title":title,"kind":"todo","time_mode":"floating","due_date":"2026-10-12","est_minutes":est}),None)).await.unwrap();
        assert_eq!(r.status, 201, "{}", r.json());
        item_ids.push(r.json()["id"].as_str().unwrap().to_owned());
    }
    let body = json!({});
    let r = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/schedule/suggest",
                body.clone(),
                Some("suggest"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200, "{}", r.json());
    assert_eq!(r.json()["suggestions"].as_array().unwrap().len(), 2);
    let replay = client
        .request(
            s.lan,
            d.request("POST", "/v1/schedule/suggest", body, Some("suggest")),
        )
        .await
        .unwrap();
    assert_eq!(r.body, replay.body);
    let suggestion = &r.json()["suggestions"][0];
    let accept = format!(
        "/v1/suggestions/{}/accept",
        suggestion["id"].as_str().unwrap()
    );
    let occ =
        sqlx::query_scalar::<_, i32>("select version from porchlight.item_occurrences where id=$1")
            .bind(
                suggestion["occurrence_id"]
                    .as_str()
                    .unwrap()
                    .parse::<uuid::Uuid>()
                    .unwrap(),
            )
            .fetch_one(&pool)
            .await
            .unwrap();
    let missing = client
        .request(s.lan, d.request("POST", &accept, json!({}), None))
        .await
        .unwrap();
    assert_eq!(missing.status, 428);
    let mut req = d.request("POST", &accept, json!({}), Some("accept"));
    req.headers_mut()
        .insert("if-match", occ.to_string().parse().unwrap());
    let accepted = client.request(s.lan, req).await.unwrap();
    assert_eq!(accepted.status, 200, "{}", accepted.json());
    assert_eq!(accepted.json()["status"], "accepted");
    let detached: bool =
        sqlx::query_scalar("select is_detached from porchlight.item_occurrences where id=$1")
            .bind(
                suggestion["occurrence_id"]
                    .as_str()
                    .unwrap()
                    .parse::<uuid::Uuid>()
                    .unwrap(),
            )
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(detached);
    let agenda: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.member_agenda where occurrence_id=$1 and busy is not null",
    )
    .bind(
        suggestion["occurrence_id"]
            .as_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(agenda, 1);
    let second = &r.json()["suggestions"][1];
    let dismiss = format!("/v1/suggestions/{}/dismiss", second["id"].as_str().unwrap());
    let mut req = d.request("POST", &dismiss, json!({}), None);
    req.headers_mut()
        .insert("if-match", second["version"].to_string().parse().unwrap());
    let dismissed = client.request(s.lan, req).await.unwrap();
    assert_eq!(dismissed.status, 200, "{}", dismissed.json());
    assert_eq!(dismissed.json()["status"], "dismissed");
    let baseline = client
        .request(
            s.lan,
            d.request(
                "GET",
                &format!("/v1/items/{}", item_ids[0]),
                Value::Null,
                None,
            ),
        )
        .await
        .unwrap();
    assert!(
        !baseline.json()["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "load")
    );
    // Keep a Tuesday window: R47 empty means untracked, a missing Monday means zero.
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,1,'09:00','17:00')").bind(s.init.household_id).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    sqlx::query("delete from porchlight.availability_windows where member_id=$1 and weekday=0")
        .bind(s.init.admin_member_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.member_load_jobs set retry_at='2026-10-05T12:00:00Z'")
        .execute(&pool)
        .await
        .unwrap();
    let drained = porchlight_api::planning::run_load_batch(
        &s.state.pool,
        "2026-10-05T12:00:00Z".parse().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(drained, 1);
    let pending: bool = sqlx::query_scalar(
        "select exists(select 1 from porchlight.member_load_jobs where requested_gen>applied_gen)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!pending);
    let item = client
        .request(
            s.lan,
            d.request(
                "GET",
                &format!("/v1/items/{}", item_ids[0]),
                Value::Null,
                None,
            ),
        )
        .await
        .unwrap();
    assert!(
        item.json()["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "load")
    );
    let list = client
        .request(s.lan, d.request("GET", "/v1/items", Value::Null, None))
        .await
        .unwrap();
    assert!(list.json()["items"].as_array().unwrap().iter().any(|i| {
        i["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "load")
    }));
}
