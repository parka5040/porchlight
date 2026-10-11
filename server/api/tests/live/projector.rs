use porchlight_api::projector;
use porchlight_db::testing::{insert_appointment, seed_household};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use std::time::Duration;
use tokio::time::{Instant, sleep, timeout};

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn projector_assigns_seqs_and_notifies(pool: PgPool) {
    let mut stream = PgListener::connect_with(&pool).await.unwrap();
    stream.listen("porchlight_stream").await.unwrap();

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(projector::run(pool.clone(), async move {
        let _ = stopped.await;
    }));
    // Let the projector subscribe before the write so the notify path is exercised.
    sleep(Duration::from_millis(300)).await;

    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let seq = loop {
        let seq: Option<i64> = sqlx::query_scalar(
            "select stream_seq from porchlight.outbox where kind = 'item.changed' and entity_id = $1",
        )
        .bind(item)
        .fetch_one(&pool)
        .await
        .unwrap();
        if let Some(seq) = seq {
            break seq;
        }
        assert!(Instant::now() < deadline, "no stream_seq within 3 s");
        sleep(Duration::from_millis(50)).await;
    };

    // A notification carrying a seq >= ours must arrive (batches may cover several rows).
    timeout(Duration::from_secs(3), async {
        loop {
            let n = stream.recv().await.unwrap();
            if n.payload().parse::<i64>().unwrap() >= seq {
                break;
            }
        }
    })
    .await
    .expect("no porchlight_stream notification");

    stop.send(()).unwrap();
    timeout(Duration::from_secs(3), handle)
        .await
        .expect("projector did not stop")
        .unwrap()
        .unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn shutdown_interrupts_a_blocked_assignment(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    insert_appointment(&pool, &h).await.unwrap();
    // Hold the assignment mutex the way an assigner does, so the projector's first drain blocks on it.
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("select from porchlight.stream_assign_mutex for update")
        .execute(&mut *holder)
        .await
        .unwrap();

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(projector::run(pool.clone(), async move {
        let _ = stopped.await;
    }));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let blocked: bool = sqlx::query_scalar(
            "select exists (select 1 from pg_stat_activity where datname = current_database() \
             and wait_event_type = 'Lock' and query like '%stream_assign_mutex%')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if blocked {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "projector never blocked on the mutex"
        );
        sleep(Duration::from_millis(20)).await;
    }

    stop.send(()).unwrap();
    timeout(Duration::from_secs(3), handle)
        .await
        .expect("projector did not stop within 3 s of shutdown")
        .unwrap()
        .unwrap();
    holder.rollback().await.unwrap();
}
