//! Single-use enrollment tokens. Consumption is conditional for the set-once guard.
use chrono::{DateTime, Utc};
use sqlx::{Executor, Postgres, Transaction};
use uuid::Uuid;
pub struct EnrollmentRow {
    pub id: Uuid,
    pub household_id: Uuid,
    pub member_id: Uuid,
    pub device_label: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
}
pub struct MemberUser {
    pub member_id: Uuid,
    pub user_id: Uuid,
    pub household_id: Uuid,
}
pub async fn find<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    hash: &[u8; 32],
) -> sqlx::Result<Option<EnrollmentRow>> {
    sqlx::query_as!(EnrollmentRow,"SELECT id,household_id,member_id,device_label,expires_at,used_at FROM porchlight.enrollment_tokens WHERE token_hash=$1",hash.as_slice()).fetch_optional(ex).await
}
pub async fn lock(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> sqlx::Result<EnrollmentRow> {
    sqlx::query_as!(EnrollmentRow,"SELECT id,household_id,member_id,device_label,expires_at,used_at FROM porchlight.enrollment_tokens WHERE id=$1 FOR UPDATE",id).fetch_one(&mut **tx).await
}
pub async fn live_member<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    id: Uuid,
) -> sqlx::Result<Option<MemberUser>> {
    sqlx::query_as!(MemberUser,"SELECT m.id AS member_id,u.id AS user_id,m.household_id FROM porchlight.household_members m JOIN porchlight.users u ON u.id=m.user_id WHERE m.id=$1 AND m.deleted_at IS NULL AND u.deleted_at IS NULL",id).fetch_optional(ex).await
}
pub async fn by_username<'e>(
    ex: impl Executor<'e, Database = Postgres>,
    household: Uuid,
    username: &str,
) -> sqlx::Result<Option<MemberUser>> {
    sqlx::query_as!(MemberUser,"SELECT m.id AS member_id,u.id AS user_id,m.household_id FROM porchlight.household_members m JOIN porchlight.users u ON u.id=m.user_id WHERE m.household_id=$1 AND u.username=$2 AND m.deleted_at IS NULL AND u.deleted_at IS NULL",household,username).fetch_optional(ex).await
}
pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    member: &MemberUser,
    label: &str,
    hash: &[u8; 32],
    created_by: Uuid,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!("INSERT INTO porchlight.enrollment_tokens(household_id,member_id,device_label,token_hash,created_by,expires_at) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id",member.household_id,member.member_id,label,hash.as_slice(),created_by,expires_at).fetch_one(&mut **tx).await
}
pub async fn consume(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE porchlight.enrollment_tokens SET used_at=now() WHERE id=$1 AND used_at IS NULL",
        id
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}
pub async fn void_resets(tx: &mut Transaction<'_, Postgres>, user: Uuid) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE porchlight.password_resets SET used_at=now() WHERE user_id=$1 AND used_at IS NULL",
        user
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}
