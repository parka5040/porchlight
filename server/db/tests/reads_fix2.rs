use porchlight_db::testing::{self, Role, pool_as, seed_household};
use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn producer_checks_occurrence_household_item_and_existence(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let other = seed_household(&pool).await.unwrap();
    let item = testing::insert_appointment(&pool, &h).await.unwrap();
    let wrong_item = testing::insert_appointment(&pool, &h).await.unwrap();
    let foreign_item = testing::insert_appointment(&pool, &other).await.unwrap();
    let mut occurrences = Vec::new();
    for parent in [item, wrong_item, foreign_item] {
        let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) select household_id,id,0 from porchlight.items where id=$1 returning id").bind(parent).fetch_one(&pool).await.unwrap();
        occurrences.push(id);
    }
    for occurrence in [occurrences[1], occurrences[2], uuid::Uuid::now_v7()] {
        let err = sqlx::query(
            "select porchlight.pl_emit_notification_event($1,$2,'reminder','item',$3,'{}',$3,$4)",
        )
        .bind(h.id)
        .bind(h.user)
        .bind(item)
        .bind(occurrence)
        .execute(&pool)
        .await
        .unwrap_err();
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("23503")
        );
        assert!(
            err.as_database_error()
                .unwrap()
                .message()
                .contains("invalid occurrence reference")
        );
    }
    sqlx::query(
        "select porchlight.pl_emit_notification_event($1,$2,'reminder','item',$3,'{}',$3,$4)",
    )
    .bind(h.id)
    .bind(h.user)
    .bind(item)
    .bind(occurrences[0])
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn append_only_delete_trigger_rejects_app_and_agent_with_delete_privilege(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("select porchlight.pl_emit_notification_event($1,$2,'reminder','household',$1)")
        .bind(h.id)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    // Test the trigger, not the real production absence of DELETE privilege.
    sqlx::query(
        "grant delete on porchlight.notification_events to porchlight_app,porchlight_agent",
    )
    .execute(&pool)
    .await
    .unwrap();
    for role in [Role::App, Role::Agent] {
        let login = pool_as(&pool, role, Some("agent:trigger-test"))
            .await
            .unwrap();
        let err = sqlx::query("delete from porchlight.notification_events")
            .execute(&login)
            .await
            .unwrap_err();
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("PL403")
        );
        assert!(
            err.as_database_error()
                .unwrap()
                .message()
                .contains("append-only")
        );
    }
}
