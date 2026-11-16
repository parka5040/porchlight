//! Member time authority. Device reports are best-effort separate transactions at the API boundary.
use crate::{
    derive::{Error, Result},
    items,
    tx::{Actor, begin_as},
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MemberTimeZone {
    pub id: Uuid,
    pub household_id: Uuid,
    pub role: String,
    pub tzid: String,
    pub tz_source: String,
    pub tz_authority_device_id: Option<Uuid>,
    pub version: i32,
}
pub async fn get(conn: &mut PgConnection, member: Uuid) -> Result<MemberTimeZone> {
    sqlx::query_as("select m.id,m.household_id,m.role,porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) as tzid,m.tz_source,m.tz_authority_device_id,m.version from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=$1 and m.deleted_at is null").bind(member).fetch_optional(conn).await?.ok_or(Error::NotFound)
}
fn validation(reason: &str) -> Error {
    porchlight_core::items::ValidationError {
        field: "tzid",
        reason: reason.into(),
    }
    .into()
}
/// Caller is authenticated already; recheck the device in this small independent transaction.
/// Only source=device and no authority OR the reporting authority can change the zone.
/// The pipeline catches every error so zone reports never fail the request.
pub async fn report_device_zone(
    pool: &PgPool,
    actor: &Actor,
    member: Uuid,
    device: Uuid,
    zone: Tz,
) -> Result<bool> {
    report_device_zone_at(pool, actor, member, device, zone, chrono::Utc::now()).await
}
/// API clock-aware report; the compatibility wrapper above retains its signature.
pub async fn report_device_zone_at(
    pool: &PgPool,
    actor: &Actor,
    member: Uuid,
    device: Uuid,
    zone: Tz,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool> {
    let (zone, _) = porchlight_core::time::storage_zone(zone.name())
        .map_err(|_| validation("unknown storage time zone"))?;
    // Unlocked no-op reports need only one read, with no actor GUC or transaction.
    // A possible change is rechecked under the existing device/scope lock protocol.
    {
        let mut conn = pool.acquire().await?;
        if !report_needed(&mut conn, member, device, zone).await? {
            return Ok(false);
        }
    }
    let mut tx = begin_as(pool, actor).await?;
    let live: Option<Uuid> = sqlx::query_scalar("select member_id from porchlight.devices where id=$1 and member_id=$2 and revoked_at is null for no key update").bind(device).bind(member).fetch_optional(&mut *tx).await?;
    if live.is_none() || !report_needed(&mut tx, member, device, zone).await? {
        tx.rollback().await?;
        return Ok(false);
    }
    let old = get(&mut tx, member).await?;
    let scope = items::WriteScope::begin_at(
        &mut tx,
        old.household_id,
        items::ScopeRequest {
            member_zones: vec![member],
            enqueue_only: true,
            ..Default::default()
        },
        now,
    )
    .await?;
    let result=sqlx::query("update porchlight.household_members m set tzid=$3,tz_authority_device_id=$2 where m.id=$1 and m.role<>'dependent' and m.tz_source='device' and (m.tz_authority_device_id is null or m.tz_authority_device_id=$2 or not exists(select 1 from porchlight.devices d where d.id=m.tz_authority_device_id and d.revoked_at is null)) and (m.tzid is distinct from $3 or m.tz_authority_device_id is distinct from $2)").bind(member).bind(device).bind(zone.name()).execute(&mut **scope.tx).await?;
    // Reports stay asynchronous; no projections are read for authority or authentication.
    drop(scope);
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}
async fn report_needed(
    conn: &mut PgConnection,
    member: Uuid,
    device: Uuid,
    zone: Tz,
) -> Result<bool> {
    sqlx::query_scalar("SELECT m.role<>'dependent' AND m.tz_source='device'
        AND EXISTS(SELECT 1 FROM porchlight.devices d WHERE d.id=$2 AND d.member_id=m.id AND d.revoked_at IS NULL)
        AND (NOT EXISTS(SELECT 1 FROM porchlight.devices a WHERE a.id=m.tz_authority_device_id AND a.member_id=m.id AND a.revoked_at IS NULL)
          OR (m.tz_authority_device_id=$2 AND porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid)<>$3))
        FROM porchlight.household_members m JOIN porchlight.households h ON h.id=m.household_id
        WHERE m.id=$1 AND m.deleted_at IS NULL")
        .bind(member).bind(device).bind(zone.name()).fetch_optional(conn).await?.ok_or(Error::NotFound)
}
async fn checked(
    scope: &mut items::WriteScope<'_, '_>,
    member: Uuid,
    version: i32,
) -> Result<MemberTimeZone> {
    let row = get(scope.tx, member).await?;
    if row.household_id != scope.household {
        return Err(Error::NotFound);
    }
    if row.version != version {
        return Err(Error::VersionConflict);
    }
    Ok(row)
}
pub async fn set_time_zone(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    member: Uuid,
    version: i32,
    zone: Tz,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<MemberTimeZone> {
    let (zone, _) = porchlight_core::time::storage_zone(zone.name())
        .map_err(|_| validation("unknown storage time zone"))?;
    let mut scope = items::WriteScope::begin_at(
        tx,
        household,
        items::ScopeRequest {
            member_zones: vec![member],
            enqueue_only: true,
            ..Default::default()
        },
        now,
    )
    .await?;
    let row = checked(&mut scope, member, version).await?;
    if row.role == "dependent" {
        return Err(validation("dependents use the household time zone"));
    }
    sqlx::query("update porchlight.household_members set tzid=$2,tz_source='manual',tz_authority_device_id=null where id=$1").bind(member).bind(zone.name()).execute(&mut **scope.tx).await?;
    let result = get(scope.tx, member).await?;
    // Zone primitives change member state and enqueue only. No post-member domain/FK
    // writes: worker or the next ordinary scope derives the affected items.
    scope.finish(now).await?;
    Ok(result)
}
/// Device must belong to this member and be live. Validation is unlocked: caller/device
/// serialization remains S1's protocol, and this primitive never locks a second device.
pub async fn make_authority(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    member: Uuid,
    version: i32,
    device: Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<MemberTimeZone> {
    let mut scope = items::WriteScope::begin_at(
        tx,
        household,
        items::ScopeRequest {
            member_zones: vec![member],
            enqueue_only: true,
            ..Default::default()
        },
        now,
    )
    .await?;
    let row = checked(&mut scope, member, version).await?;
    if row.role == "dependent" {
        return Err(validation("dependents have no authority device"));
    }
    let valid:bool=sqlx::query_scalar("select exists(select 1 from porchlight.devices where id=$1 and member_id=$2 and household_id=$3 and revoked_at is null)").bind(device).bind(member).bind(household).fetch_one(&mut **scope.tx).await?;
    if !valid {
        return Err(validation(
            "authority device must be a live device of this member",
        ));
    }
    sqlx::query("update porchlight.household_members set tz_source='device',tz_authority_device_id=$2 where id=$1").bind(member).bind(device).execute(&mut **scope.tx).await?;
    let result = get(scope.tx, member).await?;
    // Zone primitives change member state and enqueue only. No post-member domain/FK
    // writes: worker or the next ordinary scope derives the affected items.
    scope.finish(now).await?;
    Ok(result)
}
