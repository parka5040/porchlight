use chrono::{DateTime, Utc};
use porchlight_db::testing::{Role, insert_appointment, pool_as, seed_household};
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn successful_delivery_cleans_retry_metadata_once_per_batch(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let now: DateTime<Utc> = "2026-10-05T12:00:00Z".parse().unwrap();
    sqlx::raw_sql(
        "CREATE TABLE public.cleanup_calls (id int);
         CREATE FUNCTION public.count_cleanup() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
         SET search_path=pg_catalog AS $$ BEGIN INSERT INTO public.cleanup_calls VALUES (1); RETURN NULL; END $$;
         CREATE TRIGGER count_cleanup AFTER DELETE ON porchlight.notification_event_delivery
         FOR EACH STATEMENT EXECUTE FUNCTION public.count_cleanup();",
    )
    .execute(&pool)
    .await
    .unwrap();
    for _ in 0..3 {
        sqlx::query("INSERT INTO porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,created_at) VALUES ($1,$2,'assigned',$3,'item',$3,$4)")
            .bind(h.id).bind(h.super_admin).bind(item).bind(now).execute(&pool).await.unwrap();
    }
    let app = pool_as(&pool, Role::App, Some("batch-cleanup"))
        .await
        .unwrap();
    assert_eq!(porchlight_api::notifier::drain(&app, now).await.unwrap(), 3);
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM public.cleanup_calls")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(calls, 1, "successful-event cleanup must be batched");
    let processed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.notification_events WHERE processed_at IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(processed, 3);
    app.close().await;
}
