use super::acl::response_with_conflicts;
use crate::extract::Caller;
use crate::testing::{HouseholdFixture, TestOptions, trust};
use serde_json::json;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn explicit_empty_conflicts_never_recompute_pending_agenda(pool: sqlx::PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let timing = json!({"title":"overlap", "kind":"appointment", "tzid":"UTC", "starts_at":"2026-10-07T14:00:00Z", "ends_at":"2026-10-07T15:00:00Z"});
    let other = trust::admin(&f.server, "POST", "/v1/items", timing.clone()).await;
    assert_eq!(other.status, 201);
    let made = trust::admin(&f.server, "POST", "/v1/items", timing).await;
    assert_eq!(made.status, 201);
    let id: Uuid = made.json()["id"].as_str().unwrap().parse().unwrap();
    let caller = Caller::Agent {
        token_id: Uuid::nil(),
        member_id: f.admin.member_id,
        label: "test".into(),
        scopes: vec!["admin".into()],
        household_id: f.server.init.household_id,
        request_id: Uuid::now_v7(),
    };
    let actor = porchlight_db::tx::Actor {
        member_id: Some(f.admin.member_id),
        device_id: Some(f.admin.device.device_id),
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let mut tx = porchlight_db::tx::begin_as(&f.server.state.pool, &actor)
        .await
        .unwrap();
    sqlx::query("UPDATE porchlight.items SET buffer_before_min=buffer_before_min+1 WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut conn = f.server.state.pool.acquire().await.unwrap();
    let read = response_with_conflicts(&mut conn, &caller, id, false, None)
        .await
        .unwrap();
    assert!(read.pending_derivation);
    assert!(!read.conflicts.is_empty());
    let mutation = response_with_conflicts(&mut conn, &caller, id, false, Some(vec![]))
        .await
        .unwrap();
    assert!(mutation.pending_derivation);
    assert!(
        mutation.conflicts.is_empty(),
        "explicit empty mutation outcome must not recompute stale agenda"
    );
    drop(conn);
    f.server.stop().await;
}
