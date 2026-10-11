use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;

#[sqlx::test(migrations = "../db/migrations")]
async fn fold_suggestion_can_be_accepted_unchanged(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(|| "2026-11-01T04:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let member = s.init.admin_member_id;
    sqlx::query("update porchlight.household_members set tzid='America/New_York' where id=$1")
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) select household_id,id,6,'00:00','04:00' from porchlight.household_members where id=$1")
        .bind(member).execute(&pool).await.unwrap();
    let device = s.session_device(member, Platform::Desktop).await;
    let client = TestClient::new(&s.ca_pem, Some(&device));
    let created = client.request(s.lan,device.request("POST","/v1/items",json!({
        "title":"Fold work","kind":"todo","time_mode":"floating","due_date":"2026-11-01","est_minutes":120
    }),None)).await.unwrap();
    assert_eq!(created.status, 201, "{}", created.json());
    let proposed = client
        .request(
            s.lan,
            device.request("POST", "/v1/schedule/suggest", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(proposed.status, 200, "{}", proposed.json());
    let proposed = proposed.json();
    assert_eq!(proposed["suggestions"].as_array().unwrap().len(), 1);
    let suggestion = &proposed["suggestions"][0];
    let occurrence: uuid::Uuid = suggestion["occurrence_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let version: i32 =
        sqlx::query_scalar("select version from porchlight.item_occurrences where id=$1")
            .bind(occurrence)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut request = device.request(
        "POST",
        &format!(
            "/v1/suggestions/{}/accept",
            suggestion["id"].as_str().unwrap()
        ),
        json!({}),
        None,
    );
    request
        .headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let accepted = client.request(s.lan, request).await.unwrap();
    assert_eq!(
        accepted.status,
        200,
        "server must accept its unchanged fold suggestion: {}",
        accepted.json()
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn recipient_can_accept_while_aggregate_board_is_expired(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(|| "2026-10-10T12:05:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let member = s.init.admin_member_id;
    sqlx::query("update porchlight.household_members set tzid='America/Los_Angeles' where id=$1")
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
    let user:uuid::Uuid = sqlx::query_scalar("insert into porchlight.users(household_id,username) values($1,'planner-owner') returning id")
        .bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    let owner:uuid::Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,user_id,role,display_name,color,tzid) values($1,$2,'user','Sam Rivera','#8EC07C','America/New_York') returning id")
        .bind(s.init.household_id).bind(user).fetch_one(&pool).await.unwrap();
    let item:uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,local_due,est_minutes,missed_policy,recurrence_mode,rrule) values($1,$2,'Recipient work','deadline','floating','2026-10-10 08:00',30,'skip','calendar','FREQ=DAILY;COUNT=3') returning id")
        .bind(s.init.household_id).bind(owner).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(s.init.household_id)
    .bind(item)
    .bind(member)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,5,'05:00','08:00')")
        .bind(s.init.household_id).bind(member).execute(&pool).await.unwrap();
    let device = s.session_device(member, Platform::Desktop).await;
    let client = TestClient::new(&s.ca_pem, Some(&device));
    let proposed = client
        .request(
            s.lan,
            device.request("POST", "/v1/schedule/suggest", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(proposed.status, 200, "{}", proposed.json());
    let proposed = proposed.json();
    assert_eq!(proposed["suggestions"].as_array().unwrap().len(), 3);
    let suggestion = &proposed["suggestions"][0];
    let occurrence: uuid::Uuid = suggestion["occurrence_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let (version, disposition): (i32, String) =
        sqlx::query_as("select version,disposition from porchlight.item_occurrences where id=$1")
            .bind(occurrence)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(disposition, "expired");
    let mut request = device.request(
        "POST",
        &format!(
            "/v1/suggestions/{}/accept",
            suggestion["id"].as_str().unwrap()
        ),
        json!({}),
        None,
    );
    request
        .headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let accepted = client.request(s.lan, request).await.unwrap();
    assert_eq!(
        accepted.status,
        200,
        "recipient eligibility must override aggregate expiry: {}",
        accepted.json()
    );
}
