use super::*;
use crate::testing::{TestOptions, TestServer};

#[sqlx::test(migrations = "../db/migrations")]
async fn production_snapshot_keeps_mixed_anchor_reminders(pool: sqlx::PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2028-01-01T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    let item: Uuid = sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,time_mode,local_due) VALUES($1,$2,'Mixed anchors','deadline','floating','2026-01-01') RETURNING id")
        .bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    let occurrence: Uuid = sqlx::query_scalar("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq,local_due,due_at,override_local_due,override_local_start,override_local_end) VALUES($1,$2,0,'2026-01-01','2026-01-01Z','2030-01-01','2030-01-01','2030-01-02') RETURNING id")
        .bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    let reminder: Uuid = sqlx::query_scalar("INSERT INTO porchlight.item_reminders(household_id,item_id,offset_minutes) VALUES($1,$2,10) RETURNING id")
        .bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    let scheduled: Uuid = sqlx::query_scalar("INSERT INTO porchlight.scheduled_reminders(household_id,item_id,member_id,occurrence_id,reminder_id,fire_at) VALUES($1,$2,$3,$4,$5,'2028-01-01') RETURNING id")
        .bind(h.id).bind(item).bind(h.user).bind(occurrence).bind(reminder).fetch_one(&pool).await.unwrap();
    let c = Caller::Member {
        household_id: h.id,
        member_id: h.user,
        user_id: Uuid::nil(),
        role: "user".into(),
        device_id: Uuid::now_v7(),
        session_id: Uuid::nil(),
        request_id: Uuid::nil(),
        client_addr: None,
    };
    let response = snapshot(State(s.state.clone()), c).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        value["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == occurrence.to_string()),
        "production snapshot dropped mixed-anchor occurrence"
    );
    assert!(
        value["scheduled_reminders"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == scheduled.to_string()),
        "production snapshot dropped its scheduled reminder"
    );
    s.stop().await;
}
