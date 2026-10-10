//! Transactional outbox: stream sequence assignment for committed events.

/// Assigns stream sequence numbers to committed, unprojected outbox rows (oldest id first, at most `batch`).
/// Safe for concurrent callers: the assignment runs in its own transaction holding the row lock on
/// `porchlight.stream_assign_mutex` (agents cannot take the row lock, though they can queue requests that stall
/// it), so callers are serialized and commit order equals sequence order; waiting for it fails after 5 s
/// (`lock_timeout`). When anything was assigned, the transaction
/// also announces the highest sequence on `porchlight_stream` (delivered only on commit).
/// Returns the highest sequence assigned, or None if nothing was pending.
pub async fn assign_stream_seqs(
    pool: &sqlx::PgPool,
    batch: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let mut tx = crate::tx::begin(pool).await?;
    sqlx::query("set local lock_timeout = '5s'")
        .execute(&mut *tx)
        .await?;
    // fetch_one: a missing mutex row must fail loudly rather than silently skip the serialization.
    sqlx::query("select from porchlight.stream_assign_mutex for update")
        .fetch_one(&mut *tx)
        .await?;
    let seqs = sqlx::query_scalar!(
        r#"WITH pending AS (
    SELECT id FROM porchlight.outbox WHERE stream_seq IS NULL ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED
), numbered AS (
    SELECT id, nextval('porchlight.outbox_stream_seq') AS seq FROM pending
)
UPDATE porchlight.outbox o SET stream_seq = numbered.seq FROM numbered WHERE o.id = numbered.id
RETURNING o.stream_seq"#,
        batch
    )
    .fetch_all(&mut *tx)
    .await?;
    let max = seqs.into_iter().flatten().max();
    if let Some(max) = max {
        sqlx::query("select pg_notify('porchlight_stream', $1)")
            .bind(max.to_string())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(max)
}
