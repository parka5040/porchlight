//! PAT queries. Only live tokens owned by a live super admin authenticate the agent surface.
use chrono::{DateTime, Utc};
use sqlx::{Executor, Postgres, Transaction};
use uuid::Uuid;
pub struct PatIdentity {
    pub token_id: Uuid,
    pub household_id: Uuid,
    pub member_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}
pub async fn authenticate<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    hash: &[u8; 32],
) -> sqlx::Result<Option<PatIdentity>> {
    sqlx::query_as!(PatIdentity,"SELECT t.id AS token_id,t.household_id,t.owner_member_id AS member_id,u.id AS user_id,u.username,t.name,t.scopes,t.expires_at,t.last_used_at FROM porchlight.api_tokens t JOIN porchlight.household_members m ON m.id=t.owner_member_id JOIN porchlight.users u ON u.id=m.user_id WHERE t.token_hash=$1 AND t.revoked_at IS NULL AND t.expires_at>now() AND m.role='super_admin' AND m.deleted_at IS NULL AND u.deleted_at IS NULL",hash.as_slice()).fetch_optional(ex).await
}
pub async fn touch(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> sqlx::Result<()> {
    sqlx::query!("UPDATE porchlight.api_tokens SET last_used_at=now() WHERE id=$1 AND revoked_at IS NULL AND expires_at>now() AND (last_used_at IS NULL OR last_used_at<=now()-interval '1 hour')",id).execute(&mut **tx).await?;
    Ok(())
}
pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    owner: &PatIdentity,
    name: &str,
    scopes: &[String],
    hash: &[u8; 32],
    expires_at: DateTime<Utc>,
) -> sqlx::Result<Uuid> {
    create_with_id(tx, Uuid::now_v7(), owner, name, scopes, hash, expires_at).await
}
/// Shared insert for normal issuance and local recovery, which attributes the new token's id.
pub async fn create_with_id(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    owner: &PatIdentity,
    name: &str,
    scopes: &[String],
    hash: &[u8; 32],
    expires_at: DateTime<Utc>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!("INSERT INTO porchlight.api_tokens(id,household_id,owner_member_id,name,scopes,token_hash,expires_at) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id",id,owner.household_id,owner.member_id,name,scopes,hash.as_slice(),expires_at).fetch_one(&mut **tx).await
}

pub struct RecoveryOwner {
    pub household_id: Uuid,
    pub member_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
}
/// The installation and household must be live; member/user authority is held through issuance.
pub async fn recovery_owner(
    tx: &mut Transaction<'_, Postgres>,
    member: Option<Uuid>,
    username: &str,
) -> sqlx::Result<Option<RecoveryOwner>> {
    sqlx::query_as!(
        RecoveryOwner,
        "SELECT m.household_id,m.id AS member_id,u.id AS user_id,u.username
         FROM porchlight.install_state i JOIN porchlight.households h ON h.id=i.household_id
         JOIN porchlight.household_members m ON m.household_id=h.id
         JOIN porchlight.users u ON u.id=m.user_id
         WHERE (m.id=$1 OR ($1 IS NULL AND u.username=$2)) AND m.role='super_admin'
           AND h.deleted_at IS NULL AND m.deleted_at IS NULL AND u.deleted_at IS NULL
         FOR SHARE OF m,u",
        member,
        username
    )
    .fetch_optional(&mut **tx)
    .await
}
pub struct TokenSummary {
    pub id: Uuid,
    pub owner_member_id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}
pub async fn list<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    household: Uuid,
) -> sqlx::Result<Vec<TokenSummary>> {
    sqlx::query_as!(TokenSummary,"SELECT id,owner_member_id,name,scopes,created_at,expires_at,last_used_at,revoked_at FROM porchlight.api_tokens WHERE household_id=$1 ORDER BY created_at DESC,id DESC",household).fetch_all(ex).await
}
pub async fn newest_admin_expiry<'e>(
    ex: impl Executor<'e, Database = Postgres>,
) -> sqlx::Result<Option<DateTime<Utc>>> {
    sqlx::query_scalar!("SELECT t.expires_at FROM porchlight.api_tokens t
        JOIN porchlight.install_state i ON i.household_id=t.household_id
        JOIN porchlight.households h ON h.id=i.household_id
        JOIN porchlight.household_members m ON m.id=t.owner_member_id
        JOIN porchlight.users u ON u.id=m.user_id
        WHERE t.revoked_at IS NULL AND t.expires_at>now() AND 'admin'=ANY(t.scopes)
          AND m.role='super_admin' AND m.deleted_at IS NULL AND u.deleted_at IS NULL AND h.deleted_at IS NULL
        ORDER BY t.created_at DESC,t.id DESC LIMIT 1").fetch_optional(ex).await
}
/// Reserve both PATs before claiming either caller's authority. The ordered
/// wait lets a revoker join the lock queue ahead of later requests on a busy PAT,
/// without the caller->target inversion of reciprocal revocations. Call this
/// before share_pat or any other authorization/domain row lock.
pub async fn lock_revoke_pair(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    caller: Uuid,
    target: Uuid,
) -> sqlx::Result<()> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM porchlight.api_tokens WHERE household_id=$1 AND id=ANY($2) ORDER BY id FOR NO KEY UPDATE",
    )
    .bind(household)
    .bind(&[caller, target][..])
    .fetch_all(&mut **tx)
    .await?;
    Ok(())
}
pub async fn revoke(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    id: Uuid,
) -> sqlx::Result<Option<DateTime<Utc>>> {
    let found = sqlx::query_scalar!(
        // The API prelocks caller + target in UUID order before revalidation.
        // Other composed callers that already own their PAT must still fail
        // NOWAIT, not wait in caller->target order. Self-revoke is reentrant.
        "SELECT id FROM porchlight.api_tokens WHERE household_id=$1 AND id=$2 FOR NO KEY UPDATE NOWAIT",
        household,
        id
    )
    .fetch_optional(&mut **tx)
    .await?;
    if found.is_none() {
        return Ok(None);
    }
    sqlx::query!(
        "UPDATE porchlight.api_tokens SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
        id
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query_scalar!(
        "SELECT revoked_at FROM porchlight.api_tokens WHERE id=$1",
        id
    )
    .fetch_one(&mut **tx)
    .await
}

/// Hold the PAT row against revoke, and return its current authorization (not a header-time snapshot).
pub async fn share_pat(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> sqlx::Result<Option<(Uuid, Vec<String>)>> {
    let row = sqlx::query!("SELECT owner_member_id,scopes FROM porchlight.api_tokens WHERE id=$1 AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR NO KEY UPDATE", id).fetch_optional(&mut **tx).await?;
    Ok(row.map(|r| (r.owner_member_id, r.scopes)))
}
