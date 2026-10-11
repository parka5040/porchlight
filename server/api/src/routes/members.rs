//! Member self-service and Today settings. Administrative user management belongs to S5.
use super::read_models::{MemberRead, MemberReadPage, SettingsRead};
use crate::extract::ApiQuery as Query;
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
    idempotency::IdemTx,
    state::AppState,
};
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use sqlx::{Acquire, PgConnection, types::Json as SqlJson};
use uuid::Uuid;

#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TodaySettings {
    pub today_sections: Vec<TodaySection>,
    pub scope: SettingsScope,
    #[serde(deserialize_with = "week_days")]
    pub this_week_days: u8,
    pub pinned_list_id: Option<Uuid>,
}
// Integer magnitudes outside u8 are semantic range errors (422), not malformed JSON.
fn week_days<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let value = <Value as serde::Deserialize>::deserialize(d)?;
    if !value.is_i64() && !value.is_u64() {
        return Err(serde::de::Error::custom("expected integer"));
    }
    Ok(value
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .unwrap_or(0))
}
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodaySection {
    Overdue,
    Today,
    ThisWeek,
    NeedsOwner,
    PinnedList,
}
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsScope {
    Mine,
    Household,
}
impl Default for TodaySettings {
    fn default() -> Self {
        Self {
            today_sections: vec![
                TodaySection::Overdue,
                TodaySection::Today,
                TodaySection::ThisWeek,
                TodaySection::NeedsOwner,
            ],
            scope: SettingsScope::Mine,
            this_week_days: 7,
            pinned_list_id: None,
        }
    }
}
impl TodaySettings {
    pub fn validate(&self) -> Result<(), ApiError> {
        if !(1..=14).contains(&self.this_week_days) {
            return Err(validation("this_week_days", "must be 1..14"));
        }
        if self.today_sections.len() > 5
            || self
                .today_sections
                .iter()
                .enumerate()
                .any(|(n, s)| self.today_sections[..n].contains(s))
        {
            return Err(validation(
                "today_sections",
                "must contain unique section names",
            ));
        }
        if self.today_sections.contains(&TodaySection::PinnedList) && self.pinned_list_id.is_none()
        {
            return Err(validation(
                "pinned_list_id",
                "required when pinned_list section is enabled",
            ));
        }
        Ok(())
    }
}
pub fn validation(field: &str, message: &str) -> ApiError {
    ApiError::new(Problem::ValidationFailed)
        .extension("errors", json!([{"field":field,"message":message}]))
}
pub(super) const MEMBER: &str = "jsonb_build_object('id',m.id,
    'household_id',m.household_id,
    'display_name',m.display_name,
    'role',m.role,
    'color',m.color,
    'initials',upper(left(m.display_name,1) || case when strpos(trim(m.display_name),' ')>0 then left(regexp_replace(trim(m.display_name),'^.* ',''),1) else '' end),
    'tzid',porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid),
    'tz_source',m.tz_source,
    'tz_authority_device_id',case when m.id=$2 then m.tz_authority_device_id end,
    'version',m.version,
    'pending_derivation',m.id=$2 AND exists(select 1
      from porchlight.item_derivation d
      join porchlight.items i on i.id=d.item_id
      where d.applied_gen<d.requested_gen
      and i.deleted_at is null
      and i.household_id=m.household_id and (i.owner_id=m.id or i.subject_member_id=m.id
      or exists (select 1
      from porchlight.item_assignees a where a.item_id=i.id and a.member_id=m.id))))";
pub async fn all(
    conn: &mut PgConnection,
    household: Uuid,
    member: Uuid,
) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<SqlJson<Value>> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select {MEMBER}
            from porchlight.household_members m
            join porchlight.households h on h.id=m.household_id
            where m.household_id=$1 and m.deleted_at is null order by m.id"
    )))
    .bind(household)
    .bind(member)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|v| v.0).collect())
}
async fn member_row(
    conn: &mut PgConnection,
    household: Uuid,
    target: Uuid,
    caller: Uuid,
) -> Result<Value, ApiError> {
    let row: Option<SqlJson<Value>> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select {MEMBER}
            from porchlight.household_members m
            join porchlight.households h on h.id=m.household_id
            where m.household_id=$1 and m.id=$3 and m.deleted_at is null"
    )))
    .bind(household)
    .bind(caller)
    .bind(target)
    .fetch_optional(conn)
    .await?;
    row.map(|v| v.0)
        .ok_or_else(|| ApiError::new(Problem::NotFound))
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct MembersParams {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[utoipa::path(get,path="/v1/members",params(MembersParams),responses((status=200,description="Live household member profiles with effective zones",body=MemberReadPage),crate::openapi::UniversalProblems))]
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(p): Query<MembersParams>,
) -> Result<Json<crate::pagination::Page<Value>>, ApiError> {
    let limit = crate::pagination::PageParams {
        cursor: None,
        limit: p.limit,
    }
    .limit()?;
    let context = format!("members:{}:{}", caller.household_id(), caller.member_id());
    let after: Option<Uuid> = p
        .cursor
        .as_deref()
        .map(|c| state.idem.cursor.decode(&context, c))
        .transpose()?;
    let mut conn = state.pool.acquire().await?;
    let mut rows = all(&mut conn, caller.household_id(), caller.member_id()).await?;
    rows.retain(|v| after.is_none_or(|a| v["id"].as_str().unwrap().parse::<Uuid>().unwrap() > a));
    rows.truncate(limit + 1);
    Ok(Json(state.idem.cursor.page(
        &context,
        rows,
        limit,
        |v| v["id"].as_str().unwrap().parse::<Uuid>().unwrap(),
    )?))
}
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MemberPatch {
    pub color: Option<String>,
    pub tz_source: Option<String>,
    pub tzid: Option<String>,
    /// true means the authenticated calling device, never an arbitrary device id.
    pub make_zone_authority: Option<bool>,
}
fn resource(response: Value) -> Response {
    let version = response["version"].as_i64().unwrap();
    let mut r = Json(response).into_response();
    r.headers_mut()
        .insert("etag", format!("\"{version}\"").parse().unwrap());
    r
}
fn version_conflict(current: Value) -> ApiError {
    ApiError::new(Problem::VersionConflict).extension("current", current)
}
#[utoipa::path(patch,path="/v1/members/{id}",params(("id"=Uuid,Path,description="Self member id"),crate::openapi::IfMatchHeader),request_body=MemberPatch,responses((status=200,description="Updated member profile",body=MemberRead),(status=403,description="Not self or device authority requested by agent"),(status=404,description="Member missing"),(status=428,description="If-Match required",body=crate::openapi::ProblemBody,content_type="application/problem+json"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems))]
pub fn patch<'a>(
    state: &'a AppState,
    caller: Caller,
    tx: &'a mut IdemTx,
    body: MemberPatch,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        let id = tx
            .context
            .target_id
            .ok_or_else(|| ApiError::new(Problem::InvalidRequest))?;
        if id != caller.member_id() {
            return Err(ApiError::new(Problem::Forbidden));
        }
        let expected = crate::protocol::if_match(&tx.context.if_match)?;
        let authority = body.make_zone_authority.unwrap_or(false);
        if body.tz_source.as_deref().is_some_and(|v| v != "manual")
            || body.tz_source.is_some() != body.tzid.is_some()
            || (authority && body.tzid.is_some())
        {
            return Err(validation(
                "tz_source",
                "use manual with tzid, or make_zone_authority=true",
            ));
        }
        if body.color.is_none() && body.tzid.is_none() && !authority {
            return Err(validation(
                "member",
                "at least one editable field is required",
            ));
        }
        if body.color.as_deref().is_some_and(|v| {
            v.len() != 7
                || !v.starts_with('#')
                || !v[1..]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
        }) {
            return Err(validation("color", "expected uppercase #RRGGBB"));
        }
        let zone = body
            .tzid
            .as_deref()
            .map(|v| {
                porchlight_core::time::storage_zone(v)
                    .map(|(zone, _)| zone)
                    .map_err(|_| ApiError::new(Problem::InvalidTimeZone))
            })
            .transpose()?;
        let current = member_row(&mut tx.tx, caller.household_id(), id, caller.member_id()).await?;
        if current["version"] != expected {
            return Err(version_conflict(current));
        }
        let device = if authority {
            match caller {
                Caller::Member { device_id, .. } => Some(device_id),
                _ => return Err(ApiError::new(Problem::Forbidden)),
            }
        } else {
            None
        };
        let mut domain = tx.tx.begin().await?;
        let now = (state.item_clock)();
        let result: Result<(), ApiError> = async {
            if let Some(zone) = zone {
                porchlight_db::members::set_time_zone(
                    &mut domain,
                    caller.household_id(),
                    id,
                    expected,
                    zone,
                    now,
                )
                .await
                .map_err(domain_error)?;
            } else if let Some(device) = device {
                porchlight_db::members::make_authority(
                    &mut domain,
                    caller.household_id(),
                    id,
                    expected,
                    device,
                    now,
                )
                .await
                .map_err(domain_error)?;
            }
            if let Some(color) = body.color {
                let count = sqlx::query(
                    "update porchlight.household_members set color=$3
                     where household_id=$1 and id=$2 and ($4::boolean or version=$5)",
                )
                .bind(caller.household_id())
                .bind(id)
                .bind(color)
                .bind(zone.is_some() || device.is_some())
                .bind(expected)
                .execute(&mut *domain)
                .await?
                .rows_affected();
                if count == 0 {
                    return Err(ApiError::new(Problem::VersionConflict));
                }
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => domain.commit().await?,
            Err(error) => {
                domain.rollback().await?;
                if error.problem() == Problem::VersionConflict {
                    return Err(version_conflict(
                        member_row(&mut tx.tx, caller.household_id(), id, caller.member_id())
                            .await?,
                    ));
                }
                return Err(error);
            }
        }
        let current = member_row(&mut tx.tx, caller.household_id(), id, caller.member_id()).await?;
        if zone.is_some() || device.is_some() {
            // Capture exactly the member's pending jobs in the enqueue-only transaction.
            // Each candidate then uses the worker's item-first acquisition in a fresh tx.
            let items: Vec<Uuid> = sqlx::query_scalar(
                "select d.item_id from porchlight.item_derivation d
                 join porchlight.items i on i.id=d.item_id
                 where i.household_id=$1 and d.applied_gen<d.requested_gen
                 and (i.owner_id=$2 or i.subject_member_id=$2 or exists (
                     select 1 from porchlight.item_assignees a
                     where a.item_id=i.id and a.member_id=$2))
                 order by d.item_id",
            )
            .bind(caller.household_id())
            .bind(id)
            .fetch_all(&mut *tx.tx)
            .await?;
            let mut refreshed = current.clone();
            tx.after_commit(Box::new(move |state, original| {
                Box::pin(async move {
                    for item in items {
                        if crate::derive_worker::drain_candidate(&state.pool, item, now)
                            .await
                            .is_err()
                        {
                            tracing::warn!(%item, "member post-commit derivation drain deferred to worker");
                        }
                    }
                    let Ok(mut conn) = state.pool.acquire().await else {
                        return original;
                    };
                    match member_row(&mut conn, caller.household_id(), id, caller.member_id())
                        .await
                    {
                        Ok(row) => {
                            refreshed["pending_derivation"] = row["pending_derivation"].clone();
                            resource(refreshed)
                        }
                        Err(_) => original,
                    }
                })
            }));
        }
        Ok(resource(current))
    })
}
fn domain_error(e: porchlight_db::derive::Error) -> ApiError {
    use porchlight_db::derive::Error;
    match e {
        Error::Sql(e) => e.into(),
        Error::NotFound => ApiError::new(Problem::NotFound),
        Error::VersionConflict => ApiError::new(Problem::VersionConflict),
        Error::RetryableScope => ApiError::transient_conflict().retry(1),
        Error::ScopeAlreadyUsed => ApiError::internal("scope_already_used"),
        Error::Validation(error) => validation(error.field, &error.reason),
        Error::Agenda(_) | Error::Ordering(_) => ApiError::internal("member_derivation"),
    }
}
pub async fn settings_row(conn: &mut PgConnection, caller: &Caller) -> Result<Value, ApiError> {
    let row: Option<SqlJson<Value>> = sqlx::query_scalar(
        "select jsonb_build_object('member_id',member_id,'settings',settings,'version',version)
        from porchlight.member_settings where household_id=$1 and member_id=$2",
    )
    .bind(caller.household_id())
    .bind(caller.member_id())
    .fetch_optional(conn)
    .await?;
    let mut row = row.map(|v| v.0).unwrap_or_else(
        || json!({"member_id":caller.member_id(),"settings":TodaySettings::default(),"version":0}),
    );
    let normalized = serde_json::from_value::<TodaySettings>(row["settings"].clone())
        .ok()
        .filter(|settings| settings.validate().is_ok())
        .unwrap_or_default();
    row["settings"] =
        serde_json::to_value(normalized).map_err(|_| ApiError::internal("settings"))?;
    Ok(row)
}
#[utoipa::path(get,path="/v1/me/settings",responses((status=200,description="Caller Today settings and version",body=SettingsRead),crate::openapi::UniversalProblems))]
pub async fn settings_get(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Response, ApiError> {
    let mut c = state.pool.acquire().await?;
    Ok(resource(settings_row(&mut c, &caller).await?))
}
#[utoipa::path(put,path="/v1/me/settings",params(crate::openapi::IfMatchHeader),request_body=TodaySettings,responses((status=200,description="Validated Today settings and version",body=SettingsRead),(status=428,description="If-Match required",body=crate::openapi::ProblemBody,content_type="application/problem+json"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems))]
pub fn settings_put<'a>(
    _state: &'a AppState,
    caller: Caller,
    tx: &'a mut IdemTx,
    body: TodaySettings,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        let expected = crate::protocol::if_match(&tx.context.if_match)?;
        body.validate()?;
        let value = serde_json::to_value(body).map_err(|_| ApiError::internal("settings"))?;
        let count = sqlx::query(
            "update porchlight.member_settings set settings=$3
             where household_id=$1 and member_id=$2 and version=$4",
        )
        .bind(caller.household_id())
        .bind(caller.member_id())
        .bind(SqlJson(value))
        .bind(expected)
        .execute(&mut *tx.tx)
        .await?
        .rows_affected();
        if count == 0 {
            return Err(version_conflict(settings_row(&mut tx.tx, &caller).await?));
        }
        Ok(resource(settings_row(&mut tx.tx, &caller).await?))
    })
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list, patch, settings_get, settings_put),
    components(schemas(TodaySettings, TodaySection, SettingsScope, MemberPatch))
)]
pub struct MembersApi;

#[cfg(test)]
mod fix1_tests {
    use super::*;
    #[tokio::test]
    async fn validation_reason_preserved_and_derivation_failure_internal() {
        let error = domain_error(porchlight_db::derive::Error::Validation(
            porchlight_core::items::ValidationError {
                field: "tzid",
                reason: "dependents use the household time zone".into(),
            },
        ));
        assert_eq!(error.problem(), Problem::ValidationFailed);
        let bytes = axum::body::to_bytes(error.into_response().into_body(), 4096)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value["errors"][0]["message"],
            "dependents use the household time zone"
        );
        let error = domain_error(porchlight_db::derive::Error::Ordering(
            porchlight_core::ordering::OrderingError::InvalidBounds,
        ));
        assert_eq!(error.problem(), Problem::Internal);
    }
    #[cfg(feature = "testing")]
    #[sqlx::test(migrations = "../db/migrations")]
    async fn member_row_separates_target_from_caller(pool: sqlx::PgPool) {
        let s = crate::testing::TestServer::start_with(
            &pool,
            crate::testing::TestOptions {
                background_derivation: false,
                ..Default::default()
            },
        )
        .await;
        let d = s
            .session_device(
                s.init.admin_member_id,
                porchlight_core::auth::session::Platform::Desktop,
            )
            .await;
        sqlx::query(
            "update porchlight.household_members set tz_authority_device_id=$2 where id=$1",
        )
        .bind(s.init.admin_member_id)
        .bind(d.device_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'Private pending','todo','private')").bind(s.init.household_id).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let own = member_row(
            &mut conn,
            s.init.household_id,
            s.init.admin_member_id,
            s.init.admin_member_id,
        )
        .await
        .unwrap();
        assert_eq!(own["tz_authority_device_id"], d.device_id.to_string());
        assert_eq!(own["pending_derivation"], true);
        let other = member_row(
            &mut conn,
            s.init.household_id,
            s.init.admin_member_id,
            Uuid::now_v7(),
        )
        .await
        .unwrap();
        assert_eq!(other["id"], s.init.admin_member_id.to_string());
        assert!(other["tz_authority_device_id"].is_null());
        assert_eq!(other["pending_derivation"], false);
        drop(conn);
        s.stop().await;
    }
}
