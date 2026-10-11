use porchlight_api::testing::{HouseholdFixture, TestClient, TestOptions};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

fn options() -> TestOptions {
    TestOptions {
        background_derivation: false,
        item_clock: Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
        ..Default::default()
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn suggest_body_reference_validation_precedes_target_authorization(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device));
    for (member, status) in [
        (Uuid::now_v7(), 422),
        (f.users[1].member_id, 403),
        (f.dependent, 403),
    ] {
        let r = client
            .request(
                f.server.lan,
                f.users[0].device.request(
                    "POST",
                    "/v1/schedule/suggest",
                    json!({"member_id":member}),
                    None,
                ),
            )
            .await
            .unwrap();
        assert_eq!(r.status, status, "body member reference: {}", r.json());
        if status == 422 {
            assert_eq!(r.json()["errors"][0]["code"], "invalid-reference");
        }
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_marks_conflicts_unavailable_until_acceptance(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.admin.device));
    let r = client
        .request(
            f.server.lan,
            f.admin.device.request(
                "POST",
                "/v1/items",
                json!({"title":"Proposed work","due_date":"2026-10-12","est_minutes":30}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201);
    let item: Uuid = r.json()["id"].as_str().unwrap().parse().unwrap();
    let (occ, version): (Uuid, i32) =
        sqlx::query_as("select id,version from porchlight.item_occurrences where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    let id: Uuid = sqlx::query_scalar("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) values($1,$2,$3,$4,'2026-10-12T13:00Z','2026-10-12T13:30Z',$5) returning id").bind(f.server.init.household_id).bind(f.admin.member_id).bind(item).bind(occ).bind(Uuid::now_v7()).fetch_one(&pool).await.unwrap();
    let r = client
        .request(
            f.server.lan,
            f.admin
                .device
                .request("GET", "/v1/snapshot", Value::Null, None),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    let body = r.json();
    let row = body["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id.to_string())
        .unwrap();
    assert_eq!(
        row["conflicts_evaluated"], false,
        "empty conflicts must not imply a safe proposal"
    );
    let mut request = f.admin.device.request(
        "POST",
        &format!("/v1/suggestions/{id}/accept"),
        json!({}),
        None,
    );
    request
        .headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let r = client.request(f.server.lan, request).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.json());
    assert_eq!(r.json()["conflicts_evaluated"], true);
    f.server.stop().await;
}
