//! Identity and credential queries. Credential issuance locks the device row; revocation is monotonic.
use chrono::{DateTime, Utc};
use sqlx::{Executor, Postgres, Transaction};
use uuid::Uuid;

pub struct DeviceIdentity {
    pub device_id: Uuid,
    pub household_id: Uuid,
    pub member_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub role: String,
    pub platform: String,
    pub label: String,
    pub revoked: bool,
    pub certificate_live: bool,
    pub certificate_seen: bool,
}
pub struct SessionRow {
    pub id: Uuid,
    pub device_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
}
pub struct Credentials {
    pub password_hash: String,
    pub failed_attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
}
pub struct ResetRow {
    pub id: Uuid,
    pub code_hash: Vec<u8>,
    pub failed_attempts: i32,
}
pub async fn device_by_fingerprint<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    fp: &[u8; 32],
) -> sqlx::Result<Option<DeviceIdentity>> {
    sqlx::query_as!(DeviceIdentity, "SELECT d.id AS device_id,d.household_id,d.member_id,u.id AS user_id,u.username,m.role,d.platform,d.label,(d.revoked_at IS NOT NULL OR c.revoked_at IS NOT NULL) AS \"revoked!\",(c.not_after>clock_timestamp()) AS \"certificate_live!\",(c.first_seen_at IS NOT NULL) AS \"certificate_seen!\" FROM porchlight.device_certificates c JOIN porchlight.devices d ON d.id=c.device_id JOIN porchlight.household_members m ON m.id=d.member_id JOIN porchlight.users u ON u.id=m.user_id WHERE c.fingerprint_sha256=$1 AND m.deleted_at IS NULL AND u.deleted_at IS NULL",fp.as_slice()).fetch_optional(ex).await
}
/// Re-check revocation under the same row lock used by device revocation.
/// Lock order is device → session for issuance, revocation and sliding touch.
/// Sliding touch skips contended devices so it never holds a session while waiting for a device.
pub async fn lock_live_device(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> sqlx::Result<bool> {
    Ok(sqlx::query_scalar!(
        "SELECT revoked_at IS NULL AS \"live!\" FROM porchlight.devices WHERE id=$1 FOR UPDATE",
        id
    )
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(false))
}
pub async fn create_session(
    tx: &mut Transaction<'_, Postgres>,
    household_id: Uuid,
    device_id: Uuid,
    token_hash: &[u8; 32],
    expires_at: DateTime<Utc>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!("INSERT INTO porchlight.sessions(household_id,device_id,token_hash,expires_at) VALUES ($1,$2,$3,$4) RETURNING id",household_id,device_id,token_hash.as_slice(),expires_at).fetch_one(&mut **tx).await
}
pub async fn find_session<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    token_hash: &[u8; 32],
) -> sqlx::Result<Option<SessionRow>> {
    sqlx::query_as!(SessionRow,"SELECT id,device_id,expires_at,last_used_at FROM porchlight.sessions WHERE token_hash=$1 AND revoked_at IS NULL AND expires_at>now()",token_hash.as_slice()).fetch_optional(ex).await
}
/// Caller uses an actor-attributed transaction. Conditional update serializes concurrent sliding touches.
pub async fn touch_session<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    session_id: Uuid,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    // Materialize the device lock before touching the session. Contention is best-effort:
    // leave timestamps unchanged and retry on the next authenticated request.
    sqlx::query!("WITH locked AS MATERIALIZED (SELECT d.id FROM porchlight.devices d JOIN porchlight.sessions s ON s.device_id=d.id WHERE s.id=$1 AND d.revoked_at IS NULL FOR UPDATE OF d SKIP LOCKED), touched AS (UPDATE porchlight.sessions SET expires_at=$2,last_used_at=now() WHERE id=$1 AND device_id IN (SELECT id FROM locked) AND revoked_at IS NULL AND expires_at>now() AND last_used_at<=now()-interval '1 hour' RETURNING device_id) UPDATE porchlight.devices SET last_seen_at=now() WHERE id IN (SELECT device_id FROM touched)",session_id,expires_at).execute(ex).await?;
    Ok(())
}
pub async fn revoke_session<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    session_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE porchlight.sessions SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
        session_id
    )
    .execute(ex)
    .await?;
    Ok(())
}
pub async fn revoke_device_sessions<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    device_id: Uuid,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE porchlight.sessions SET revoked_at=now() WHERE device_id=$1 AND revoked_at IS NULL",
        device_id
    )
    .execute(ex)
    .await?
    .rows_affected())
}
pub async fn revoke_user_sessions<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    user_id: Uuid,
    except: Option<Uuid>,
) -> sqlx::Result<u64> {
    Ok(sqlx::query!("UPDATE porchlight.sessions SET revoked_at=now() WHERE revoked_at IS NULL AND ($2::uuid IS NULL OR id<>$2) AND device_id IN (SELECT d.id FROM porchlight.devices d JOIN porchlight.household_members m ON m.id=d.member_id WHERE m.user_id=$1)",user_id,except).execute(ex).await?.rows_affected())
}
pub async fn lock_credentials(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> sqlx::Result<Option<Credentials>> {
    sqlx::query_as!(Credentials,"SELECT password_hash,failed_attempts,locked_until FROM porchlight.user_credentials WHERE user_id=$1 FOR UPDATE",user_id).fetch_optional(&mut **tx).await
}
pub async fn record_failure(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    failed_attempts: i32,
    locked_until: Option<DateTime<Utc>>,
) -> sqlx::Result<()> {
    sqlx::query!("UPDATE porchlight.user_credentials SET failed_attempts=$2,locked_until=$3 WHERE user_id=$1",user_id,failed_attempts,locked_until).execute(&mut **tx).await?;
    Ok(())
}
pub async fn record_success(tx: &mut Transaction<'_, Postgres>, user_id: Uuid) -> sqlx::Result<()> {
    record_failure(tx, user_id, 0, None).await
}
pub async fn set_password(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    phc: &str,
) -> sqlx::Result<()> {
    sqlx::query!("UPDATE porchlight.user_credentials SET password_hash=$2,password_changed_at=now(),failed_attempts=0,locked_until=NULL WHERE user_id=$1",user_id,phc).execute(&mut **tx).await?;
    Ok(())
}
pub async fn create_password_reset(
    tx: &mut Transaction<'_, Postgres>,
    household_id: Uuid,
    user_id: Uuid,
    code_hash: &[u8; 32],
    created_by_member: Uuid,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!("INSERT INTO porchlight.password_resets(household_id,user_id,code_hash,created_by,expires_at) VALUES ($1,$2,$3,$4,$5) RETURNING id",household_id,user_id,code_hash.as_slice(),created_by_member,expires_at).fetch_one(&mut **tx).await
}
pub async fn lock_active_reset(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> sqlx::Result<Option<ResetRow>> {
    sqlx::query_as!(ResetRow,"SELECT id,code_hash,failed_attempts FROM porchlight.password_resets WHERE user_id=$1 AND used_at IS NULL AND expires_at>now() AND failed_attempts<5 ORDER BY created_at DESC,id DESC LIMIT 1 FOR UPDATE",user_id).fetch_optional(&mut **tx).await
}
pub async fn record_reset_failure(
    tx: &mut Transaction<'_, Postgres>,
    reset_id: Uuid,
) -> sqlx::Result<i32> {
    sqlx::query_scalar!("UPDATE porchlight.password_resets SET failed_attempts=failed_attempts+1 WHERE id=$1 AND used_at IS NULL RETURNING failed_attempts",reset_id).fetch_one(&mut **tx).await
}
pub async fn consume_reset(tx: &mut Transaction<'_, Postgres>, reset_id: Uuid) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE porchlight.password_resets SET used_at=now() WHERE id=$1 AND used_at IS NULL",
        reset_id
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}
pub async fn create_step_up(
    tx: &mut Transaction<'_, Postgres>,
    household_id: Uuid,
    session_id: Uuid,
    device_id: Uuid,
    action: &str,
    token_hash: &[u8; 32],
    expires_at: DateTime<Utc>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!("INSERT INTO porchlight.step_ups(household_id,session_id,device_id,action,token_hash,expires_at) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id",household_id,session_id,device_id,action,token_hash.as_slice(),expires_at).fetch_one(&mut **tx).await
}
pub async fn consume_step_up(
    tx: &mut Transaction<'_, Postgres>,
    token_hash: &[u8; 32],
    session_id: Uuid,
    device_id: Uuid,
    action: &str,
) -> sqlx::Result<bool> {
    Ok(sqlx::query!("UPDATE porchlight.step_ups SET used_at=now() WHERE token_hash=$1 AND session_id=$2 AND device_id=$3 AND action=$4 AND used_at IS NULL AND expires_at>now()",token_hash.as_slice(),session_id,device_id,action).execute(&mut **tx).await?.rows_affected()==1)
}
pub async fn display_name<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    member: Uuid,
) -> sqlx::Result<String> {
    sqlx::query_scalar!(
        "SELECT display_name FROM porchlight.household_members WHERE id=$1 AND deleted_at IS NULL",
        member
    )
    .fetch_one(ex)
    .await
}

/// Serialize mutations on the device before checking sessions or locking credentials.
/// Sessions are checked unlocked: holding a session lock across credentials can deadlock
/// with password changes on another device which revoke all of the user's sessions.
/// See docs/specs/lock-order.md. Never locks households.
pub async fn share_device(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> sqlx::Result<Option<bool>> {
    sqlx::query_scalar!(
        "SELECT revoked_at IS NULL AS \"live!\" FROM porchlight.devices WHERE id=$1 FOR NO KEY UPDATE",
        id
    )
    .fetch_optional(&mut **tx)
    .await
}
pub async fn share_session(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    device: Uuid,
) -> sqlx::Result<bool> {
    Ok(sqlx::query_scalar!("SELECT revoked_at IS NULL AND expires_at>clock_timestamp() AND device_id=$2 AS \"live!\" FROM porchlight.sessions WHERE id=$1", id, device).fetch_optional(&mut **tx).await?.unwrap_or(false))
}
pub async fn share_member(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar!("SELECT m.role FROM porchlight.household_members m JOIN porchlight.users u ON u.id=m.user_id WHERE m.id=$1 AND m.deleted_at IS NULL AND u.deleted_at IS NULL", id).fetch_optional(&mut **tx).await
}
