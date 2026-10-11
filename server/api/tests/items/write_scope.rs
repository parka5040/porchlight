use chrono::{DateTime, Utc};
use porchlight_api::derive_worker;
use porchlight_db::{
    derive, items,
    testing::{Role, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}
async fn write(app: &PgPool, household: Uuid, id: Uuid) {
    let mut tx = begin_as(app, &Actor::system()).await.unwrap();
    let mut scope = items::WriteScope::begin(
        &mut tx,
        household,
        items::ScopeRequest {
            items: vec![id],
            ..Default::default()
        },
    )
    .await
    .expect("scope must progress without 40P01");
    let row = scope.item(id).await.unwrap();
    let mut fields = items::ItemWrite::from(&row);
    fields.notes.push('x');
    scope.update(id, row.version, &fields).await.unwrap();
    scope
        .finish(now())
        .await
        .expect("finish must progress without 40P01");
    tx.commit().await.unwrap();
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn overlapping_and_disjoint_scope_worker_stress_twenty_both_orders(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let a = insert_appointment(&pool, &h).await.unwrap();
    let b = insert_appointment(&pool, &h).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    for disjoint in [false, true] {
        let owner = if disjoint { h.user } else { h.super_admin };
        sqlx::query("update porchlight.items set owner_id=$2 where id=$1")
            .bind(b)
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
        for reverse in [false, true] {
            for _ in 0..20 {
                // Raw enqueue before each pair forces overlapping peers into both plans.
                sqlx::query("update porchlight.items set buffer_before_min=1-buffer_before_min where id=any($1)").bind(vec![a,b]).execute(&pool).await.unwrap();
                sqlx::query("update porchlight.item_derivation set retry_at=$1")
                    .bind(now())
                    .execute(&pool)
                    .await
                    .unwrap();
                let (left, right) = if reverse { (b, a) } else { (a, b) };
                tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    let worker = derive_worker::run_batch(&app, now());
                    let (_, _, run) =
                        tokio::join!(write(&app, h.id, left), write(&app, h.id, right), worker);
                    run.expect("worker must progress without 40P01");
                })
                .await
                .expect("concurrent scopes/worker stalled");
                let pending:i64=sqlx::query_scalar("select count(*) from porchlight.item_derivation where item_id=any($1) and applied_gen<requested_gen").bind(vec![a,b]).fetch_one(&pool).await.unwrap();
                assert_eq!(pending, 0);
            }
        }
    }
    let notes: Vec<String> =
        sqlx::query_scalar("select notes from porchlight.items where id=any($1)")
            .bind(vec![a, b])
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(notes.iter().all(|n| n.len() == 80));
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn worker_skips_held_scope_and_disjoint_write_commits_before_release(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let a = insert_appointment(&pool, &h).await.unwrap();
    let b = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set owner_id=$2 where id=$1")
        .bind(b)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, b, now()).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query("update porchlight.item_derivation set retry_at=$1")
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            items: vec![a],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let skipped = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        derive_worker::run_batch(&app, now()),
    )
    .await
    .expect("worker waited instead of SKIP LOCKED")
    .unwrap();
    assert_eq!(skipped, 0);
    tokio::time::timeout(std::time::Duration::from_secs(2), write(&app, h.id, b))
        .await
        .expect("disjoint scope blocked by household-wide locks");
    let row = scope.item(a).await.unwrap();
    let mut fields = items::ItemWrite::from(&row);
    fields.title = "held edit".into();
    scope.update(a, row.version, &fields).await.unwrap();
    scope.finish(now()).await.unwrap();
    tx.commit().await.unwrap();
    let pending: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_derivation where applied_gen<requested_gen",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
}
