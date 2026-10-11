//! Transactions attributed to an actor.

/// Who is performing a write. Recorded by triggers via transaction-local settings.
#[derive(Clone, Debug)]
pub struct Actor {
    pub member_id: Option<uuid::Uuid>,
    pub device_id: Option<uuid::Uuid>,
    pub request_id: uuid::Uuid,
    pub agent_label: Option<String>,
    pub token_id: Option<uuid::Uuid>,
}

impl Actor {
    /// Authenticated app caller. A member and device always travel together.
    pub fn member(member_id: uuid::Uuid, device_id: uuid::Uuid, request_id: uuid::Uuid) -> Self {
        Self {
            member_id: Some(member_id),
            device_id: Some(device_id),
            request_id,
            agent_label: None,
            token_id: None,
        }
    }
    /// HTTP PAT caller, attributed to its authenticated owner and token name.
    /// Unlike raw SQL's validated `agent:<name>` application_name, the HTTP label
    /// is the token's stored name verbatim; token_id records the channel identity.
    pub fn pat(
        member_id: uuid::Uuid,
        token_id: uuid::Uuid,
        label: String,
        request_id: uuid::Uuid,
    ) -> Self {
        Self {
            member_id: Some(member_id),
            device_id: None,
            request_id,
            agent_label: Some(label),
            token_id: Some(token_id),
        }
    }

    /// Background work has no member/device/PAT identity; each transaction gets a request id.
    pub fn system() -> Self {
        Self {
            member_id: None,
            device_id: None,
            request_id: uuid::Uuid::now_v7(),
            agent_label: None,
            token_id: None,
        }
    }
}

/// Begins a top-level transaction on `pool`. Every non-test top-level transaction opens here or
/// in [`begin_as`]; `scripts/ci.sh` rejects sqlx's own begin calls elsewhere. Any async caller
/// can be cancelled (a client disconnect drops its handler, a timeout or `select!` drops work).
///
/// sqlx's pool `begin()` is not cancel-safe: dropped after BEGIN is sent but before its reply is
/// read, it neither counts nor rolls back the transaction, so the pool hands out a connection
/// that is still inside it. The next "autocommit" statement there then stays uncommitted, and
/// the next `SET TRANSACTION` fails (SQLSTATE 25001). Here only the (cancel-safe) acquisition
/// waits in the caller; BEGIN runs in a detached task that always finishes, and if the caller is
/// gone the task drops the transaction, which queues its ROLLBACK before the connection returns
/// to the pool. Savepoints (`tx.begin()`) have a different hazard: see R86 in `begin_as`.
pub async fn begin(
    pool: &sqlx::PgPool,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    use tracing::Instrument;
    let conn = pool.acquire().await?;
    let begin = sqlx::Transaction::begin(conn, None); // tx-guard: allow (the cancel-safe wrapper)
    match tokio::spawn(begin.instrument(tracing::Span::current())).await {
        Ok(tx) => tx,
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(_) => Err(sqlx::Error::WorkerCrashed),
    }
}

/// Begins a transaction whose writes are attributed to `actor` (settings are transaction-local, so they never
/// leak to the next user of a pooled connection).
///
/// R86: a transaction whose inner work was cancelled (a timeout, `select!` or drop around code
/// using it) is poisoned. A savepoint `tx.begin()` dropped mid-flight makes sqlx queue a full
/// ROLLBACK and forget the transaction, so later statements autocommit without its locks or these
/// actor settings, and `commit()`/`rollback()` return `Ok` without sending anything. Never reuse
/// it: drop it and do any follow-up bookkeeping in a fresh `begin_as` transaction that re-takes
/// its locks.
pub async fn begin_as(
    pool: &sqlx::PgPool,
    actor: &Actor,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    let mut tx = begin(pool).await?;
    sqlx::query(
        "select set_config('porchlight.actor_member_id', $1, true), \
         set_config('porchlight.device_id', $2, true), \
         set_config('porchlight.request_id', $3, true), \
         set_config('porchlight.agent_label', $4, true), \
         set_config('porchlight.token_id', $5, true)",
    )
    .bind(actor.member_id.map(|u| u.to_string()).unwrap_or_default())
    .bind(actor.device_id.map(|u| u.to_string()).unwrap_or_default())
    .bind(actor.request_id.to_string())
    .bind(actor.agent_label.as_deref().unwrap_or_default())
    .bind(actor.token_id.map(|u| u.to_string()).unwrap_or_default())
    .execute(&mut *tx)
    .await?;
    Ok(tx)
}
