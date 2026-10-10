//! Claim-first replay storage; the caller commits claim, effects and response in one transaction.
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    Device,
    Pat,
    Enrollment,
}
impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Pat => "pat",
            Self::Enrollment => "enrollment",
        }
    }
}
pub struct StoredResponse {
    pub request_hash: Vec<u8>,
    pub status: Option<i32>,
    pub sealed: Option<Vec<u8>>,
}
pub enum ClaimOutcome {
    Claimed(Uuid),
    Existing(StoredResponse),
}
pub async fn claim(
    tx: &mut Transaction<'_, Postgres>,
    household_id: Uuid,
    kind: ScopeKind,
    scope_id: Uuid,
    key: &str,
    request_hash: &[u8; 32],
    ttl: chrono::Duration,
) -> sqlx::Result<ClaimOutcome> {
    let kind = kind.as_str();
    sqlx::query!("DELETE FROM porchlight.idempotency_keys WHERE scope_kind=$1 AND scope_id=$2 AND idem_key=$3 AND expires_at<=now()",kind,scope_id,key).execute(&mut **tx).await?;
    let seconds = ttl.num_seconds() as f64;
    let id=sqlx::query_scalar!("INSERT INTO porchlight.idempotency_keys(household_id,scope_kind,scope_id,idem_key,request_hash,expires_at) VALUES ($1,$2,$3,$4,$5,now()+$6::float8*interval '1 second') ON CONFLICT(scope_kind,scope_id,idem_key) DO NOTHING RETURNING id",household_id,kind,scope_id,key,request_hash.as_slice(),seconds).fetch_optional(&mut **tx).await?;
    if let Some(id) = id {
        return Ok(ClaimOutcome::Claimed(id));
    }
    let stored=sqlx::query_as!(StoredResponse,"SELECT request_hash,response_status AS status,response_sealed AS sealed FROM porchlight.idempotency_keys WHERE scope_kind=$1 AND scope_id=$2 AND idem_key=$3",kind,scope_id,key).fetch_one(&mut **tx).await?;
    Ok(ClaimOutcome::Existing(stored))
}
pub async fn complete(
    tx: &mut Transaction<'_, Postgres>,
    claim_id: Uuid,
    status: u16,
    sealed: &[u8],
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE porchlight.idempotency_keys SET response_status=$2,response_sealed=$3 WHERE id=$1",
        claim_id,
        i32::from(status),
        sealed
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}
pub async fn purge_expired(pool: &PgPool) -> sqlx::Result<u64> {
    Ok(
        sqlx::query!("DELETE FROM porchlight.idempotency_keys WHERE expires_at<=now()")
            .execute(pool)
            .await?
            .rows_affected(),
    )
}
