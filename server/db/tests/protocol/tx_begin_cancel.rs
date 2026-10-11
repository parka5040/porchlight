//! A caller dropped while its BEGIN is in flight (a client disconnect cancelling a handler or the
//! household export, a timeout) must not hand the next pooled user a connection that is still
//! inside the abandoned transaction.
use porchlight_db::{
    testing::{Role, connect_options_as},
    tx,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};
use uuid::Uuid;

const APP: &str = "porchlight-begin-cancel";

/// Drops `tx::begin` at the unsafe point: its BEGIN has executed on the server, but the caller
/// has not consumed the reply. Polled by hand and gated on the server's own state, so there is
/// no timing assumption; returns false when the begin completed first (the caller retries).
async fn drop_once_begin_executed(app: &PgPool, monitor: &PgPool) -> bool {
    let mut begin = Box::pin(tx::begin(app));
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..10_000 {
        if let Poll::Ready(tx) = begin.as_mut().poll(&mut cx) {
            tx.unwrap().rollback().await.unwrap();
            return false;
        }
        let executed: bool = sqlx::query_scalar(
            "select exists(select 1 from pg_stat_activity where datname=current_database() \
             and application_name=$1 and state='idle in transaction' and query='BEGIN')",
        )
        .bind(APP)
        .fetch_one(monitor)
        .await
        .unwrap();
        if executed {
            drop(begin);
            return true;
        }
    }
    panic!("BEGIN never executed");
}

#[sqlx::test(migrations = "./migrations")]
async fn dropped_begin_never_leaves_its_transaction_on_the_pooled_connection(pool: PgPool) {
    // One connection, so the next pooled statement necessarily reuses the abandoned one.
    let app = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options_as(&pool, Role::App, Some(APP)).unwrap())
        .await
        .unwrap();
    let mut dropped = false;
    for _ in 0..50 {
        if drop_once_begin_executed(&app, &pool).await {
            dropped = true;
            break;
        }
    }
    assert!(dropped, "BEGIN was never observed in flight");
    // An autocommit statement on the reused connection commits (here the export's own lease)...
    let lease: Uuid = sqlx::query_scalar("select porchlight.pl_take_blob_lease('begin-cancel')")
        .fetch_one(&app)
        .await
        .unwrap();
    let visible: bool =
        sqlx::query_scalar("select exists(select 1 from porchlight.blob_leases where id=$1)")
            .bind(lease)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        visible,
        "autocommit statement ran inside the abandoned transaction"
    );
    // ...and the next transaction is a fresh one: the export's first statement is accepted.
    let mut tx = tx::begin(&app).await.unwrap();
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}
