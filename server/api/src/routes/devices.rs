//! Device lifecycle. Self-revoke is terminal: retry authentication/handshake fails instead of replaying success.
use crate::{
    error::ApiError,
    extract::{Mutation, SessionAuth, actor},
    idempotency::{self, Begin, IdemScope},
    ratelimit::Class,
    server::ConnInfo,
    state::AppState,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use chrono::DateTime;
use chrono::{Duration, Utc};
use porchlight_core::auth::token::{TokenKind, hash_token, looks_like};
use porchlight_db::{auth, devices, idempotency::ScopeKind};
use serde::Deserialize;
use uuid::Uuid;
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Certificate {
    csr: String,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
pub(crate) async fn store_certificate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    household_id: Uuid,
    device_id: Uuid,
    cert: &porchlight_pki::IssuedCert,
) -> Result<(), ApiError> {
    devices::add_certificate(
        tx,
        devices::Certificate {
            household_id,
            device_id,
            fingerprint: &cert.fingerprint,
            serial: &cert.serial_hex,
            not_before: cert.not_before.into(),
            not_after: cert.not_after.into(),
        },
    )
    .await?;
    Ok(())
}
async fn start<T>(
    state: &AppState,
    s: &SessionAuth,
    info: &ConnInfo,
    m: &Mutation<T>,
) -> Result<Begin, ApiError> {
    state.limits.check(
        Class::Write,
        Some(s.device.device_id),
        info.client_addr.map(|a| a.ip()),
    )?;
    idempotency::begin(
        state,
        &actor(&s.device, s.request_id),
        &m.auth,
        IdemScope {
            kind: ScopeKind::Device,
            scope_id: s.device.device_id,
            household_id: s.device.household_id,
        },
        &m.key,
        &m.request_hash,
    )
    .await
}
#[utoipa::path(post,
    path="/v1/devices/self/certificate",
    request_body=Certificate,
    responses((status=200,description="Success",body=crate::routes::devices::CertificateResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem),
    security(("session" = [])))]
pub async fn certificate(
    State(state): State<AppState>,
    s: SessionAuth,
    Extension(info): Extension<ConnInfo>,
    m: Mutation<Certificate>,
) -> Result<Response, ApiError> {
    let begin = start(&state, &s, &info, &m).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        if !auth::lock_live_device(&mut tx.tx, s.device.device_id).await? {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::DeviceRevoked).into_response(),
            ));
        }
        let fp = info
            .client_cert_fingerprint
            .ok_or_else(|| ApiError::new(crate::error::Problem::Unauthenticated))?;
        let not_after = devices::not_after(&mut tx.tx, &fp, s.device.device_id).await?;
        if not_after - Utc::now() > Duration::days(i64::from(state.settings.device_renew_days)) {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::RenewalTooEarly).into_response(),
            ));
        }
        if porchlight_pki::validate_csr(&m.body.csr).is_err() {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::CsrInvalid).into_response(),
            ));
        }
        let cert = state
            .pki
            .ca
            .issue_device(
                &m.body.csr,
                &s.device.device_id.to_string(),
                std::time::SystemTime::now(),
                state.settings.device_cert_days,
            )
            .map_err(|_| ApiError::internal("renewal_certificate_issue"))?;
        store_certificate(&mut tx.tx, s.device.household_id, s.device.device_id, &cert).await?;
        let not_after: chrono::DateTime<Utc> = cert.not_after.into();
        Ok((
            tx,
            Json(CertificateResponse {
                certificate: cert.cert_pem,
                not_after,
            })
            .into_response(),
        ))
    })
    .await
}
#[utoipa::path(get,
    path="/v1/devices",
    params(("limit" = Option<u16>, Query, description = "Default 50; maximum 200"),("cursor" = Option<String>, Query, description = "Opaque signed keyset cursor")),
    responses((status=200,description="Success",body=crate::routes::devices::DevicesResponse),crate::openapi::UniversalProblems),
    security(("session" = [])))]
pub async fn list(
    State(state): State<AppState>,
    s: SessionAuth,
    page: Result<
        axum::extract::Query<crate::pagination::PageParams>,
        axum::extract::rejection::QueryRejection,
    >,
) -> Result<Json<DevicesResponse>, ApiError> {
    let axum::extract::Query(page) =
        page.map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
    let limit = page.limit()?;
    let member = (s.device.role != "super_admin").then_some(s.device.member_id);
    let context = format!("devices:{}:{:?}", s.device.household_id, member);
    let after = page
        .cursor
        .as_deref()
        .map(|c| state.idem.cursor.decode(&context, c))
        .transpose()?;
    let rows = devices::list_page(
        &state.pool,
        s.device.household_id,
        member,
        Some(s.device.device_id),
        false,
        after,
        (limit + 1) as i64,
    )
    .await?;
    let page = state
        .idem
        .cursor
        .page(&context, rows, limit, |r| (r.created_at, r.id))?;
    Ok(Json(DevicesResponse {
        devices: page.items.iter().cloned().map(Into::into).collect(),
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next_cursor,
    }))
}
#[utoipa::path(get,
    path="/v1/endpoints",
    responses((status=200,description="Success",body=crate::routes::devices::EndpointsResponse),crate::openapi::UniversalProblems),
    security(("session" = [])))]
pub async fn endpoints(State(state): State<AppState>, _: SessionAuth) -> Json<EndpointsResponse> {
    Json(EndpointsResponse {
        endpoints: state.settings.endpoints.clone(),
    })
}
#[utoipa::path(post,
    path="/v1/devices/{id}/revoke",
    request_body=Empty,
    params(("id" = String, Path, description = "Resource identifier"),crate::openapi::StepUpHeader),
    responses((status=200,description="Success",body=crate::routes::devices::RevocationResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::StepUpProblem,crate::openapi::NotFoundProblem),
    security(("session" = [])))]
pub async fn revoke(
    State(state): State<AppState>,
    s: SessionAuth,
    Extension(info): Extension<ConnInfo>,
    Path(id): Path<String>,
    headers: HeaderMap,
    m: Mutation<Empty>,
) -> Result<Response, ApiError> {
    let id =
        Uuid::parse_str(&id).map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
    let begin = start(&state, &s, &info, &m).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        let member = (s.device.role != "super_admin").then_some(s.device.member_id);
        if !devices::accessible(&mut tx.tx, s.device.household_id, member, id).await? {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::NotFound).into_response(),
            ));
        }
        let token = headers
            .get("porchlight-step-up")
            .and_then(|h| h.to_str().ok())
            .filter(|t| looks_like(TokenKind::StepUp, t));
        let valid = if let Some(token) = token {
            auth::consume_step_up(
                &mut tx.tx,
                &hash_token(token),
                s.session_id,
                s.device.device_id,
                "device.revoke",
            )
            .await?
        } else {
            false
        };
        if !valid {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::StepUpRequired).into_response(),
            ));
        }
        if id != s.device.device_id {
            devices::lock_cross_revoke(&mut tx.tx, s.device.household_id, member, id).await?; // Shared mapping logs the SQLSTATE and preserves the transient marker.
        }
        let Some(revocation) =
            devices::revoke(&mut tx.tx, s.device.household_id, member, id).await?
        else {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::NotFound).into_response(),
            ));
        };
        tx.publish_revocation(id, revocation.fingerprints);
        Ok((
            tx,
            Json(RevocationResponse {
                revoked_at: revocation.revoked_at,
            })
            .into_response(),
        ))
    })
    .await
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(certificate, list, revoke, endpoints))]
pub struct DevicesApi;

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct CertificateResponse {
    pub certificate: String,
    pub not_after: DateTime<Utc>,
}
#[derive(Clone, serde::Serialize, utoipa::ToSchema)]
pub struct Device {
    pub id: Uuid,
    pub member_id: Uuid,
    pub label: String,
    pub platform: String,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// Expiry of the latest issued certificate, or null when revoked/no certificate.
    pub certificate_expires_at: Option<DateTime<Utc>>,
    pub current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct DevicesResponse {
    pub devices: Vec<Device>,
    pub items: Vec<Device>,
    pub next_cursor: Option<String>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct EndpointsResponse {
    pub endpoints: Vec<String>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct RevocationResponse {
    pub revoked_at: DateTime<Utc>,
}
impl From<porchlight_db::devices::DeviceRow> for Device {
    fn from(r: porchlight_db::devices::DeviceRow) -> Self {
        Self {
            id: r.id,
            member_id: r.member_id,
            label: r.label,
            platform: r.platform,
            revoked_at: r.revoked_at,
            last_seen_at: r.last_seen_at,
            created_at: r.created_at,
            certificate_expires_at: r.certificate_expires_at,
            current: r.current,
            username: r.username,
        }
    }
}
