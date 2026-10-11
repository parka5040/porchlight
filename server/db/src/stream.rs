//! ID-only tickles, routed from authoritative ACLs, never NOTIFY payloads.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub const BATCH: i64 = 500;
pub const RETENTION_DAYS: i64 = 7;
#[derive(Debug, Serialize, Deserialize)]
pub struct Tickle {
    pub seq: i64,
    pub kind: String,
    pub entity_type: String,
    pub entity_id: Uuid,
    /// An access-change tickle for a former reader is an eviction, not a fetch hint.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub evicted: bool,
}
pub struct Batch {
    pub epoch: i64,
    /// Highest discarded sequence, in the same snapshot as the rows/ACL decisions.
    pub pruned_through: i64,
    pub cursor: i64,
    pub scanned: i64,
    pub events: Vec<Tickle>,
}
pub async fn maximum(pool: &PgPool) -> sqlx::Result<i64> {
    sqlx::query_scalar!("SELECT coalesce(max(stream_seq),0) AS \"max!\" FROM porchlight.outbox")
        .fetch_one(pool)
        .await
}
/// Global bounds match the snapshot's global publication cursor. Gaps are legal;
/// pruning before the first retained row conservatively requests a new snapshot.
#[derive(sqlx::FromRow, Debug)]
pub struct Window {
    pub epoch: i64,
    pub maximum: i64,
    pub expired_through: i64,
    pub first: Option<i64>,
}
impl Window {
    pub fn expired(&self, after: i64) -> bool {
        // A fresh snapshot on an idle household may have a weeks-old maximum.
        // It is still safe: there are no newer changes to miss. Avoid a resnapshot loop.
        if after == self.maximum {
            return false;
        }
        (self.expired_through > 0 && after <= self.expired_through)
            || self.first.is_some_and(|first| after < first - 1)
    }
}
pub async fn window_at(pool: &PgPool, now: DateTime<Utc>) -> sqlx::Result<Window> {
    sqlx::query_as("SELECT (SELECT epoch FROM porchlight.stream_retention) AS epoch, coalesce(max(stream_seq),0) AS maximum, coalesce(max(stream_seq) FILTER (WHERE created_at < $1),0) AS expired_through, min(stream_seq) AS first FROM porchlight.outbox")
        .bind(now - chrono::Duration::days(RETENTION_DAYS)).fetch_one(pool).await
}
/// Recheck all still-queued IDs in ONE ACL snapshot, immediately before a body flush.
/// The caller emits the entire result in one chunk: no authorized events survive a yield.
pub async fn deliveries(
    pool: &PgPool,
    household: Uuid,
    member: Uuid,
    seqs: &[i64],
) -> sqlx::Result<Vec<Tickle>> {
    assert!(seqs.len() <= BATCH as usize, "bounded stream flush");
    Ok(
        read_range(pool, household, member, 0, i64::MAX, BATCH, Some(seqs))
            .await?
            .events,
    )
}
/// O2a: retention generation and queued deliveries share one database snapshot.
pub async fn delivery_batch(
    pool: &PgPool,
    household: Uuid,
    member: Uuid,
    seqs: &[i64],
) -> sqlx::Result<Batch> {
    assert!(seqs.len() <= BATCH as usize);
    read_range(pool, household, member, 0, i64::MAX, BATCH, Some(seqs)).await
}
pub async fn retention_epoch(pool: &PgPool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT epoch FROM porchlight.stream_retention")
        .fetch_one(pool)
        .await
}
/// Single-ID convenience for non-stream callers/tests; live streams use deliveries.
pub async fn delivery(
    pool: &PgPool,
    household: Uuid,
    member: Uuid,
    seq: i64,
) -> sqlx::Result<Option<Tickle>> {
    Ok(deliveries(pool, household, member, &[seq]).await?.pop())
}
/// Limit the *scan*, not the delivered rows, so a fully filtered batch advances.
/// A single statement gives ACL decisions and watermark in one database snapshot.
pub async fn read(pool: &PgPool, household: Uuid, member: Uuid, after: i64) -> sqlx::Result<Batch> {
    read_range(pool, household, member, after, i64::MAX, BATCH, None).await
}
async fn read_range(
    pool: &PgPool,
    household: Uuid,
    member: Uuid,
    after: i64,
    through: i64,
    limit: i64,
    seqs: Option<&[i64]>,
) -> sqlx::Result<Batch> {
    let row = sqlx::query!(r#"
WITH batch AS MATERIALIZED (
    SELECT stream_seq, kind, entity_type, entity_id, household_id, payload
    FROM porchlight.outbox
    WHERE household_id=$1 AND stream_seq>$3 AND stream_seq<=$5
        AND ($6::bigint[] IS NULL OR stream_seq = ANY($6)) ORDER BY stream_seq LIMIT $4
), scoped AS (
    SELECT b.*, i.id AS item_id,
        CASE WHEN pg_input_is_valid(b.payload->>'list_id','uuid') THEN (b.payload->>'list_id')::uuid END AS list_id,
        porchlight.pl_item_can_read(i.id,$2) AS visible
    FROM batch b
    LEFT JOIN porchlight.item_occurrences o ON b.kind='occurrence.changed'
        AND b.entity_type='occurrence' AND o.id=b.entity_id AND o.household_id=$1
    -- A lateral point lookup prevents a global household scan/hash join for
    -- an OR catalogue. OFFSET 0 keeps the UUID predicate parameterized per row.
    LEFT JOIN LATERAL (
        SELECT id FROM porchlight.items WHERE household_id=$1 AND id = CASE
        WHEN b.kind IN ('item.changed','item.access_changed','derived') AND b.entity_type='item' THEN b.entity_id
        WHEN b.kind='occurrence.changed' AND b.entity_type='occurrence' THEN o.item_id
        WHEN b.kind IN ('list.changed','list_entry.changed','comment.changed','attachment.changed') THEN
            CASE WHEN pg_input_is_valid(b.payload->>'item_id','uuid') THEN (b.payload->>'item_id')::uuid END
    END OFFSET 0
    ) i ON true
), routed AS (
    SELECT *, CASE
        WHEN kind='member.changed' AND entity_type='member' THEN true
        WHEN kind IN ('notification.created','notification.updated','notification.dismissed',
            'settings.changed','ics_source.changed','suggestion.changed') THEN payload->>'member_id'=$2::text
        WHEN kind IN ('item.changed','derived') AND entity_type='item' THEN visible
        WHEN kind='occurrence.changed' AND entity_type='occurrence' THEN visible
        WHEN kind='item.access_changed' AND entity_type='item' THEN
            visible OR payload->'recipients' @> jsonb_build_array($2::text) OR payload->>'member_id'=$2::text
        WHEN (kind='list.changed' AND entity_type='list') OR (kind='list_entry.changed' AND entity_type='list_entry') THEN
            (CASE WHEN payload ? 'item_id' THEN visible
                  ELSE porchlight.pl_list_can_read(list_id,$2) END)
            OR (kind='list.changed' AND payload->>'eviction'='true'
                AND payload->'recipients' @> jsonb_build_array($2::text))
        WHEN (kind='comment.changed' AND entity_type='comment') OR (kind='attachment.changed' AND entity_type='attachment') THEN visible
        ELSE false END AS allowed
    FROM scoped
), live_member AS (
    SELECT id FROM porchlight.household_members WHERE id=$2 AND household_id=$1 AND deleted_at IS NULL AND role<>'dependent'
)
SELECT (SELECT epoch FROM porchlight.stream_retention) AS "epoch!",
       (SELECT pruned_through FROM porchlight.stream_retention) AS "pruned_through!",
       (SELECT coalesce(max(stream_seq),$3) FROM batch) AS "cursor!",
       (SELECT count(*) FROM batch) AS "scanned!",
       coalesce(jsonb_agg(jsonb_build_object('seq',stream_seq,'kind',kind,
            'entity_type',entity_type,'entity_id',entity_id,
            'evicted',(kind='item.access_changed' AND NOT coalesce(visible,false))
                OR (kind='list.changed' AND payload->>'eviction'='true' AND NOT coalesce(
                    CASE WHEN payload ? 'item_id' THEN visible ELSE porchlight.pl_list_can_read(list_id,$2) END,false))) ORDER BY stream_seq),'[]'::jsonb)
            AS "events!: sqlx::types::Json<Vec<Tickle>>"
FROM routed WHERE allowed AND EXISTS(SELECT 1 FROM live_member)
"#, household, member, after, limit, through, seqs).fetch_one(pool).await?;
    Ok(Batch {
        epoch: row.epoch,
        pruned_through: row.pruned_through,
        cursor: row.cursor,
        scanned: row.scanned,
        events: row.events.0,
    })
}
