use super::*;

#[sqlx::test(migrations = "../db/migrations")]
async fn member_zone_source_is_observed_pending_before_worker(pool: PgPool) {
    let mut m = fixture(pool, 1).await;
    m.mutate("create_shared").await;
    m.drain().await;
    checked_step(&mut m, "member_zone_source").await.unwrap();
    let pending: bool = sqlx::query_scalar(
        "select requested_gen>applied_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(m.items[0])
    .fetch_one(&m.pool)
    .await
    .unwrap();
    assert!(!pending, "worker phase did not complete the zone enqueue");
    m.s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unrelated_privacy_failure_is_not_a_notification_kill(pool: PgPool) {
    let m = fixture(pool, 1).await;
    let private = Uuid::now_v7();
    let notification = Uuid::now_v7();
    let view = View {
        bundles: BTreeMap::new(),
        inbox: json!({"items":[]}),
        settings: json!([{}, {}, {}, {"items":[]}]),
    };
    let frames = vec![
        json!({"kind":"item.invalidated","entity_type":"item","entity_id":private}),
        json!({"kind":"notification.updated","entity_type":"notification","entity_id":notification}),
    ];
    let error = verify(&m, 0, "actor_rename/source", &view, &view, &frames, false).unwrap_err();
    assert!(
        !mutations::matches_failure(
            &error,
            "actor_rename/source",
            FailureKind::Notification,
            notification
        ),
        "unrelated privacy failure counted as notification kill"
    );
    assert_eq!(error.kind, FailureKind::PrivateHint);
    assert_eq!(error.id, private);
    m.s.stop().await;
}
