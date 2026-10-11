//! Device lifecycle. Credential issuance and revocation serialize on the same device row.
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub async fn revoked_fingerprints(pool: &PgPool) -> sqlx::Result<Vec<[u8; 32]>> {
    let rows=sqlx::query!("SELECT c.fingerprint_sha256 FROM porchlight.device_certificates c JOIN porchlight.devices d ON d.id=c.device_id WHERE d.revoked_at IS NOT NULL OR c.revoked_at IS NOT NULL").fetch_all(pool).await?;
    rows.into_iter()
        .map(|r| decode(r.fingerprint_sha256))
        .collect()
}
fn decode(fp: Vec<u8>) -> sqlx::Result<[u8; 32]> {
    fp.try_into()
        .map_err(|_| sqlx::Error::Decode("invalid certificate fingerprint length".into()))
}
pub async fn revoked_device_ids(pool: &PgPool) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar!("SELECT id FROM porchlight.devices WHERE revoked_at IS NOT NULL")
        .fetch_all(pool)
        .await
}
pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    household: Uuid,
    member: Uuid,
    label: &str,
    platform: &str,
) -> sqlx::Result<()> {
    sqlx::query!("INSERT INTO porchlight.devices(id,household_id,member_id,label,platform) VALUES ($1,$2,$3,$4,$5)",id,household,member,label,platform).execute(&mut **tx).await?;
    Ok(())
}
pub struct Certificate<'a> {
    pub household_id: Uuid,
    pub device_id: Uuid,
    pub fingerprint: &'a [u8; 32],
    pub serial: &'a str,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
}
pub async fn add_certificate(
    tx: &mut Transaction<'_, Postgres>,
    cert: Certificate<'_>,
) -> sqlx::Result<()> {
    sqlx::query!("INSERT INTO porchlight.device_certificates(household_id,device_id,fingerprint_sha256,serial,not_before,not_after) VALUES ($1,$2,$3,$4,$5,$6)",cert.household_id,cert.device_id,cert.fingerprint.as_slice(),cert.serial,cert.not_before,cert.not_after).execute(&mut **tx).await?;
    Ok(())
}
pub async fn not_after(
    tx: &mut Transaction<'_, Postgres>,
    fp: &[u8; 32],
    device: Uuid,
) -> sqlx::Result<DateTime<Utc>> {
    sqlx::query_scalar!("SELECT not_after FROM porchlight.device_certificates WHERE fingerprint_sha256=$1 AND device_id=$2",fp.as_slice(),device).fetch_one(&mut **tx).await
}
/// Called under the live device lock. Replay is resolved before this check.
pub async fn renewal_available(
    tx: &mut Transaction<'_, Postgres>,
    device: Uuid,
    fp: &[u8; 32],
) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT (SELECT count(*) FROM porchlight.device_certificates WHERE device_id=$1 AND created_at>clock_timestamp()-interval '24 hours') < 3 AND NOT EXISTS (SELECT 1 FROM porchlight.device_certificates newer JOIN porchlight.device_certificates presented ON presented.device_id=newer.device_id WHERE presented.device_id=$1 AND presented.fingerprint_sha256=$2 AND newer.first_seen_at IS NOT NULL AND (newer.created_at,newer.id)>(presented.created_at,presented.id))")
        .bind(device).bind(fp.as_slice()).fetch_one(&mut **tx).await
}
/// Serialized with issuance under the device lock; never mark a superseded leaf as used.
pub async fn mark_certificate_seen(
    tx: &mut Transaction<'_, Postgres>,
    fp: &[u8; 32],
) -> sqlx::Result<()> {
    sqlx::query("UPDATE porchlight.device_certificates SET first_seen_at=clock_timestamp() WHERE fingerprint_sha256=$1 AND first_seen_at IS NULL AND revoked_at IS NULL")
        .bind(fp.as_slice()).execute(&mut **tx).await?;
    Ok(())
}
/// Replacing a lost reply revokes only confirmed-unused leaves, not legacy unknowns.
/// All issuances still count toward the budget. Caller holds the live-device lock
/// and has already checked renewal_available.
pub async fn revoke_unused_successors(
    tx: &mut Transaction<'_, Postgres>,
    device: Uuid,
    fp: &[u8; 32],
) -> sqlx::Result<()> {
    sqlx::query("UPDATE porchlight.device_certificates newer SET revoked_at=clock_timestamp() FROM porchlight.device_certificates presented WHERE presented.device_id=$1 AND presented.fingerprint_sha256=$2 AND newer.device_id=presented.device_id AND (newer.created_at,newer.id)>(presented.created_at,presented.id) AND newer.usage_tracked AND newer.first_seen_at IS NULL AND newer.revoked_at IS NULL")
        .bind(device).bind(fp.as_slice()).execute(&mut **tx).await?;
    Ok(())
}
#[derive(Clone, Serialize)]
pub struct DeviceRow {
    pub id: Uuid,
    pub member_id: Uuid,
    pub label: String,
    pub platform: String,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub certificate_expires_at: Option<DateTime<Utc>>,
    pub current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}
pub async fn list_page(
    pool: &PgPool,
    household: Uuid,
    member: Option<Uuid>,
    current: Option<Uuid>,
    admin: bool,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> sqlx::Result<Vec<DeviceRow>> {
    let (after_time, after_id) = after
        .map(|(t, id)| (Some(t), Some(id)))
        .unwrap_or((None, None));
    let mut rows=sqlx::query_as!(DeviceRow,"SELECT d.id,d.member_id,d.label,d.platform,d.revoked_at,d.last_seen_at,d.created_at,(SELECT c.not_after FROM porchlight.device_certificates c WHERE c.device_id=d.id AND d.revoked_at IS NULL ORDER BY c.created_at DESC,c.id DESC LIMIT 1) AS \"certificate_expires_at?\",coalesce(d.id=$3,false) AS \"current!\",u.username AS \"username?\" FROM porchlight.devices d JOIN porchlight.household_members m ON m.id=d.member_id LEFT JOIN porchlight.users u ON u.id=m.user_id WHERE d.household_id=$1 AND ($2::uuid IS NULL OR d.member_id=$2) AND ($4::timestamptz IS NULL OR (d.created_at,d.id)>($4,$5::uuid)) ORDER BY d.created_at,d.id LIMIT $6",household,member,current,after_time,after_id,limit).fetch_all(pool).await?;
    if !admin {
        for r in &mut rows {
            r.username = None;
        }
    }
    Ok(rows)
}
pub struct Revocation {
    pub revoked_at: DateTime<Utc>,
    pub fingerprints: Vec<[u8; 32]>,
}
/// App cross-revoke must never wait for another caller's device while holding its own.
/// A failure aborts the transaction; callers map 55P03 to retryable conflict, never seal it.
pub async fn lock_cross_revoke(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    member: Option<Uuid>,
    id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query_scalar!("SELECT id FROM porchlight.devices WHERE household_id=$1 AND ($2::uuid IS NULL OR member_id=$2) AND id=$3 FOR UPDATE NOWAIT",household,member,id).fetch_optional(&mut **tx).await?;
    Ok(())
}
/// Lock before revoking sessions or reading certificates. Repeated revocation retains the first time.
/// Permission filtering is part of the lookup (404 for an inaccessible id).
pub async fn revoke(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    member: Option<Uuid>,
    id: Uuid,
) -> sqlx::Result<Option<Revocation>> {
    let found=sqlx::query_scalar!("SELECT id FROM porchlight.devices WHERE household_id=$1 AND ($2::uuid IS NULL OR member_id=$2) AND id=$3 FOR UPDATE",household,member,id).fetch_optional(&mut **tx).await?;
    if found.is_none() {
        return Ok(None);
    }
    sqlx::query!(
        "UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
        id
    )
    .execute(&mut **tx)
    .await?;
    crate::auth::revoke_device_sessions(&mut **tx, id).await?;
    let revoked_at = sqlx::query_scalar!(
        "SELECT revoked_at AS \"revoked_at!\" FROM porchlight.devices WHERE id=$1",
        id
    )
    .fetch_one(&mut **tx)
    .await?;
    let fingerprints = sqlx::query_scalar!(
        "SELECT fingerprint_sha256 FROM porchlight.device_certificates WHERE device_id=$1",
        id
    )
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(decode)
    .collect::<sqlx::Result<_>>()?;
    Ok(Some(Revocation {
        revoked_at,
        fingerprints,
    }))
}
/// Check permissions without taking session locks (used before consuming step-up).
pub async fn accessible(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    member: Option<Uuid>,
    id: Uuid,
) -> sqlx::Result<bool> {
    Ok(sqlx::query_scalar!("SELECT id FROM porchlight.devices WHERE household_id=$1 AND ($2::uuid IS NULL OR member_id=$2) AND id=$3",household,member,id).fetch_optional(&mut **tx).await?.is_some())
}
