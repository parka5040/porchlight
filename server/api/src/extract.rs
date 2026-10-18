//! Authentication runs on every request, before any replay lookup. Secret-bearing bodies never implement Debug.
use crate::{error::ApiError, server::ConnInfo, state::AppState};
use axum::{
    body::to_bytes,
    extract::{FromRequest, FromRequestParts, Request},
    http::{header, request::Parts},
};
use chrono::{DateTime, Utc};
use porchlight_core::auth::{
    session::{Platform, SESSION_TOUCH_INTERVAL_SECS, session_lifetime},
    token::{TokenKind, hash_token, looks_like},
};
use porchlight_db::{
    auth::{self, DeviceIdentity},
    tx::{Actor, begin_as},
};
use serde::de::DeserializeOwned;
use uuid::Uuid;
/// Carried from header authentication through the body read to the transaction claim.
mod mutation_auth {
    use super::Uuid;
    #[derive(Clone)]
    pub enum MutationAuth {
        Device {
            id: Uuid,
            fingerprint: [u8; 32],
            role: String,
            session: Option<Uuid>,
            role_sensitive: bool,
        },
        Pat {
            id: Uuid,
            admin: bool,
            write: bool,
        },
        Enrollment,
    }
}
#[cfg(feature = "testing")]
pub use mutation_auth::MutationAuth;
#[cfg(not(feature = "testing"))]
pub(crate) use mutation_auth::MutationAuth;
pub struct DeviceAuth(pub DeviceIdentity, pub Uuid);
pub struct SessionAuth {
    pub device: DeviceIdentity,
    pub session_id: Uuid,
    pub(crate) session_hash: [u8; 32],
    pub session_expires_at: DateTime<Utc>,
    pub request_id: Uuid,
}
pub fn actor(device: &DeviceIdentity, request_id: Uuid) -> Actor {
    Actor::member(device.member_id, device.device_id, request_id)
}
impl FromRequestParts<AppState> for DeviceAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let fp = parts
            .extensions
            .get::<ConnInfo>()
            .and_then(|i| i.client_cert_fingerprint)
            .ok_or_else(|| ApiError::new(crate::error::Problem::Unauthenticated))?;
        let mut device = auth::device_by_fingerprint(&state.pool, &fp)
            .await?
            .ok_or_else(|| ApiError::new(crate::error::Problem::DeviceUnknown))?;
        if !device.certificate_seen && !device.revoked && device.certificate_live {
            // First use and unused-successor replacement serialize on the same device row.
            let mut tx = begin_as(&state.pool, &actor(&device, request_id(parts))).await?;
            auth::share_device(&mut tx, device.device_id).await?;
            device = auth::device_by_fingerprint(&mut *tx, &fp)
                .await?
                .ok_or_else(|| ApiError::new(crate::error::Problem::DeviceUnknown))?;
            if !device.revoked && device.certificate_live {
                porchlight_db::devices::mark_certificate_seen(&mut tx, &fp).await?;
            }
            tx.commit().await?;
        }
        if device.revoked {
            return Err(ApiError::new(crate::error::Problem::DeviceRevoked));
        }
        if !device.certificate_live {
            return Err(ApiError::new(
                crate::error::Problem::DeviceCertificateExpired,
            ));
        }
        parts.extensions.insert(MutationAuth::Device {
            id: device.device_id,
            fingerprint: fp,
            role: device.role.clone(),
            session: None,
            role_sensitive: true,
        });
        Ok(Self(device, request_id(parts)))
    }
}
impl FromRequestParts<AppState> for SessionAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let DeviceAuth(device, request_id) = DeviceAuth::from_request_parts(parts, state).await?;
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .ok_or_else(|| ApiError::new(crate::error::Problem::Unauthenticated))?;
        let token = header
            .to_str()
            .ok()
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map(|(_, token)| token)
            .filter(|t| looks_like(TokenKind::Session, t))
            .ok_or_else(|| ApiError::new(crate::error::Problem::SessionInvalid))?;
        let hash = hash_token(token);
        let mut session = auth::find_session(&state.pool, &hash)
            .await?
            .ok_or_else(|| ApiError::new(crate::error::Problem::SessionInvalid))?;
        if session.device_id != device.device_id {
            return Err(ApiError::new(crate::error::Problem::SessionDeviceMismatch));
        }
        if (Utc::now() - session.last_used_at).num_seconds() >= SESSION_TOUCH_INTERVAL_SECS {
            let platform = Platform::parse(&device.platform)
                .ok_or_else(|| ApiError::internal("session_invalid_platform"))?;
            let mut tx = begin_as(&state.pool, &actor(&device, request_id)).await?;
            auth::touch_session(
                &mut *tx,
                session.id,
                Utc::now() + session_lifetime(platform),
            )
            .await?;
            session = auth::find_session(&mut *tx, &hash)
                .await?
                .ok_or_else(|| ApiError::new(crate::error::Problem::SessionInvalid))?;
            tx.commit().await?;
        }
        if let Some(MutationAuth::Device {
            session: stored, ..
        }) = parts.extensions.get_mut::<MutationAuth>()
        {
            *stored = Some(session.id);
        }
        let terminal = terminal_for(parts, state, device.device_id, false).await;
        if let Some(MutationAuth::Device { role_sensitive, .. }) =
            parts.extensions.get_mut::<MutationAuth>()
        {
            *role_sensitive = !terminal;
        }
        // S3: a zone report is its own small transaction before the mutation claim.
        // Never retain its locks and never turn a reporting failure into a request failure.
        // GETs also report: defer authoritative changes until rollback is disarmed.
        if let Some(zone) = parts
            .extensions
            .get::<crate::protocol::ClientTz>()
            .and_then(|tz| tz.0)
            && !state.maintenance.active()
            && porchlight_db::members::report_device_zone_at(
                &state.pool,
                &actor(&device, request_id),
                device.member_id,
                device.device_id,
                zone,
                (state.item_clock)(),
            )
            .await
            .is_err()
        {
            tracing::warn!("device time-zone report failed; request continues");
        }
        Ok(Self {
            device,
            session_id: session.id,
            session_hash: hash,
            session_expires_at: session.expires_at,
            request_id,
        })
    }
}
async fn terminal_for(parts: &mut Parts, state: &AppState, id: Uuid, agent: bool) -> bool {
    match parts.extensions.get::<crate::routes::Terminal>().copied() {
        Some(crate::routes::Terminal::Always) => true,
        Some(crate::routes::Terminal::DeviceTarget) if !agent => {
            target_id(parts, state).await == Some(id)
        }
        Some(crate::routes::Terminal::PatTarget) if agent => {
            target_id(parts, state).await == Some(id)
        }
        _ => false,
    }
}
async fn target_id(parts: &mut Parts, state: &AppState) -> Option<Uuid> {
    let axum::extract::Path(params) = axum::extract::Path::<
        std::collections::HashMap<String, String>,
    >::from_request_parts(parts, state)
    .await
    .ok()?;
    params
        .get("id")
        .and_then(|value| Uuid::parse_str(value).ok())
}
#[derive(Clone, Copy)]
struct RequestId(Uuid);
fn request_id(parts: &mut Parts) -> Uuid {
    if let Some(id) = parts.extensions.get::<RequestId>() {
        return id.0;
    }
    let id = Uuid::now_v7();
    parts.extensions.insert(RequestId(id));
    id
}
/// PAT scope hierarchy: every authenticated live token reads; admin implies write.
/// Authentication/revalidation owns liveness, this function only owns scope policy.
pub fn pat_allows_scope(scopes: &[String], required: &str) -> bool {
    match required {
        "read" => true,
        "write" => scopes.iter().any(|s| s == "write" || s == "admin"),
        "admin" => scopes.iter().any(|s| s == "admin"),
        _ => false,
    }
}
/// PAT identity is fetched on every request, including terminal self-revoke retries.
pub struct PatAuth(pub porchlight_db::tokens::PatIdentity, pub Uuid);
impl PatAuth {
    pub fn require_admin(&self) -> Result<(), ApiError> {
        if pat_allows_scope(&self.0.scopes, "admin") {
            Ok(())
        } else {
            Err(ApiError::new(crate::error::Problem::Forbidden))
        }
    }
    pub fn actor(&self) -> Actor {
        Actor::pat(
            self.0.member_id,
            self.0.token_id,
            self.0.name.clone(),
            self.1,
        )
    }
}
impl FromRequestParts<AppState> for PatAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if !parts
            .extensions
            .get::<ConnInfo>()
            .is_some_and(|i| matches!(i.transport, crate::server::Transport::Agent))
        {
            return Err(ApiError::new(crate::error::Problem::Unauthenticated));
        }
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.split_once(' '))
            .filter(|(s, _)| s.eq_ignore_ascii_case("Bearer"))
            .map(|(_, t)| t)
            .filter(|t| looks_like(TokenKind::Pat, t))
            .ok_or_else(|| ApiError::new(crate::error::Problem::Unauthenticated))?;
        let hash = hash_token(token);
        let mut identity = porchlight_db::tokens::authenticate(&state.pool, &hash)
            .await?
            .ok_or_else(|| ApiError::new(crate::error::Problem::Unauthenticated))?;
        if identity
            .last_used_at
            .is_none_or(|t| (Utc::now() - t).num_seconds() >= SESSION_TOUCH_INTERVAL_SECS)
        {
            let pat = Self(identity, request_id(parts));
            let mut tx = begin_as(&state.pool, &pat.actor()).await?;
            porchlight_db::tokens::touch(&mut tx, pat.0.token_id).await?;
            identity = porchlight_db::tokens::authenticate(&mut *tx, &hash)
                .await?
                .ok_or_else(|| ApiError::new(crate::error::Problem::Unauthenticated))?;
            tx.commit().await?;
        }
        parts.extensions.insert(MutationAuth::Pat {
            id: identity.token_id,
            admin: false,
            write: false,
        });
        Ok(Self(identity, request_id(parts)))
    }
}
/// Scope check precedes path/body extraction on all privileged agent routes.
pub struct AdminAuth(pub PatAuth);
impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let pat = PatAuth::from_request_parts(parts, state).await?;
        let terminal = terminal_for(parts, state, pat.0.token_id, true).await;
        pat.require_admin()?;
        if let Some(MutationAuth::Pat { admin, .. }) = parts.extensions.get_mut::<MutationAuth>() {
            *admin = !terminal;
        }
        Ok(Self(pat))
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BodyPolicy {
    Json,
    Calendar,
}
pub struct Mutation<T> {
    pub(crate) auth: MutationAuth,
    pub key: String,
    pub request_hash: [u8; 32],
    /// Non-secret operation metadata for path/header-aware domain callbacks.
    pub context: crate::idempotency::RequestContext,
    pub body: T,
    pub request_id: Uuid,
    /// Calendar uploads only: dropped with the mutation, after the handler and commit.
    pub(crate) _body_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}
/// Validate visible ASCII against the database's CHECK (not arbitrary Unicode).
pub fn idempotency_key(headers: &axum::http::HeaderMap) -> Result<String, ApiError> {
    let mut values = headers.get_all("idempotency-key").iter();
    let value = values
        .next()
        .ok_or_else(|| ApiError::new(crate::error::Problem::IdempotencyKeyRequired))?;
    if values.next().is_some() {
        return Err(ApiError::new(crate::error::Problem::IdempotencyKeyRequired));
    }
    let value = value
        .to_str()
        .map_err(|_| ApiError::new(crate::error::Problem::IdempotencyKeyRequired))?;
    if value.is_empty() || value.len() > 255 || !value.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(ApiError::new(crate::error::Problem::IdempotencyKeyRequired));
    }
    Ok(value.to_owned())
}
pub(crate) async fn mutation_metadata(
    req: Request,
    state: &AppState,
) -> Result<
    (
        Request,
        MutationAuth,
        String,
        crate::idempotency::RequestContext,
        Uuid,
    ),
    ApiError,
> {
    let (mut parts, body) = req.into_parts();
    let request_id = request_id(&mut parts);
    let target_id = target_id(&mut parts, state).await;
    let req = Request::from_parts(parts, body);
    let auth = req
        .extensions()
        .get::<MutationAuth>()
        .cloned()
        .or_else(|| {
            req.extensions()
                .get::<ConnInfo>()
                .filter(|i| matches!(i.transport, crate::server::Transport::Enrollment))
                .map(|_| MutationAuth::Enrollment)
        })
        .ok_or_else(|| ApiError::new(crate::error::Problem::Unauthenticated))?;
    let key = idempotency_key(req.headers())?;
    let mut if_match = axum::http::HeaderMap::new();
    for value in req.headers().get_all(header::IF_MATCH) {
        if_match.append(header::IF_MATCH, value.clone());
    }
    let context = crate::idempotency::RequestContext {
        uri: req.uri().clone(),
        target_id,
        if_match,
    };
    Ok((req, auth, key, context, request_id))
}
impl<T: DeserializeOwned + Send> FromRequest<AppState> for Mutation<T> {
    type Rejection = ApiError;
    async fn from_request(req: Request, state: &AppState) -> Result<Self, ApiError> {
        let (req, auth, key, context, request_id) = mutation_metadata(req, state).await?;
        let method = req.method().as_str().to_owned();
        let path = req
            .uri()
            .path_and_query()
            .map_or(req.uri().path(), |p| p.as_str())
            .to_owned();
        let calendar_upload = req.extensions().get::<BodyPolicy>() == Some(&BodyPolicy::Calendar);
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let text_calendar =
            calendar_upload && matches!(content_type.as_str(), "text/calendar" | "text/plain");
        if content_type != "application/json" && !text_calendar {
            return Err(ApiError::new(crate::error::Problem::InvalidRequest));
        }
        // Admit before buffering: concurrent multi-MiB bodies must not exhaust server memory.
        let body_permit = if calendar_upload {
            Some(
                tokio::time::timeout(
                    state.upload_limits.permit_wait,
                    state.calendar_body_slots.clone().acquire_owned(),
                )
                .await
                .map_err(|_| ApiError::transient_conflict().retry(1))?
                .map_err(|_| ApiError::internal("calendar_body_slot"))?,
            )
        } else {
            None
        };
        // Raw text is the content itself; only the JSON envelope pays for string escaping.
        let limit = if text_calendar {
            crate::calendar::fetch::MAX_BYTES + 64 * 1024
        } else if calendar_upload {
            crate::calendar::fetch::MAX_BYTES * 6 + 64 * 1024
        } else {
            64 * 1024
        };
        #[cfg(feature = "testing")]
        state.mutation_body_started.notify_waiters();
        let bytes = tokio::time::timeout(
            state.settings.body_timeout,
            to_bytes(req.into_body(), limit),
        )
        .await
        .map_err(|_| ApiError::new(crate::error::Problem::RequestTimeout))?
        .map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
        let request_hash = state.idem.request_mac(&method, &path, &bytes);
        let body = if text_calendar {
            let axum::extract::Query(query) =
                axum::extract::Query::<crate::calendar::ImportQuery>::try_from_uri(&context.uri)
                    .map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
            let content = String::from_utf8(bytes.to_vec())
                .map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
            serde_json::from_value(
                serde_json::to_value(crate::calendar::ImportRequest {
                    content,
                    member_id: query.member_id,
                    name: query.name,
                    scope: query.scope,
                    preview: query.preview,
                    visibility: query.visibility,
                })
                .map_err(|_| ApiError::internal("ics_upload_encode"))?,
            )
            .map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?
        };
        Ok(Self {
            auth,
            key,
            request_hash,
            context,
            body,
            request_id,
            _body_permit: body_permit,
        })
    }
}

/// One attributed caller for routes mounted on either mTLS or the agent socket.
#[derive(Clone, Debug)]
pub enum Caller {
    Member {
        member_id: Uuid,
        user_id: Uuid,
        role: String,
        device_id: Uuid,
        session_id: Uuid,
        household_id: Uuid,
        client_addr: Option<std::net::IpAddr>,
        request_id: Uuid,
    },
    Agent {
        token_id: Uuid,
        member_id: Uuid,
        label: String,
        scopes: Vec<String>,
        household_id: Uuid,
        request_id: Uuid,
    },
}
impl Caller {
    pub fn actor(&self) -> Actor {
        match self {
            Self::Member {
                member_id,
                device_id,
                request_id,
                ..
            } => Actor::member(*member_id, *device_id, *request_id),
            Self::Agent {
                member_id,
                token_id,
                label,
                request_id,
                ..
            } => Actor::pat(*member_id, *token_id, label.clone(), *request_id),
        }
    }
    pub fn household_id(&self) -> Uuid {
        match self {
            Self::Member { household_id, .. } | Self::Agent { household_id, .. } => *household_id,
        }
    }
    pub fn sees_all(&self) -> bool {
        matches!(self, Self::Agent { .. })
    }
    pub fn member_id(&self) -> Uuid {
        match self {
            Self::Member { member_id, .. } | Self::Agent { member_id, .. } => *member_id,
        }
    }
    pub fn require_admin(&self) -> Result<(), ApiError> {
        let permitted = match self {
            Self::Member { role, .. } => role == "super_admin",
            Self::Agent { scopes, .. } => pat_allows_scope(scopes, "admin"),
        };
        if permitted {
            Ok(())
        } else {
            Err(ApiError::new(crate::error::Problem::Forbidden))
        }
    }
}
impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let request_id = request_id(parts);
        let client_addr = parts
            .extensions
            .get::<ConnInfo>()
            .and_then(|c| c.client_addr)
            .map(|a| a.ip());
        match parts.extensions.get::<ConnInfo>().map(|c| c.transport) {
            Some(crate::server::Transport::Agent) => {
                let PatAuth(p, _) = PatAuth::from_request_parts(parts, state).await?;
                Ok(Self::Agent {
                    token_id: p.token_id,
                    member_id: p.member_id,
                    label: p.name,
                    scopes: p.scopes,
                    household_id: p.household_id,
                    request_id,
                })
            }
            Some(crate::server::Transport::Lan | crate::server::Transport::Funnel) => {
                let s = SessionAuth::from_request_parts(parts, state).await?;
                Ok(Self::Member {
                    member_id: s.device.member_id,
                    user_id: s.device.user_id,
                    role: s.device.role,
                    device_id: s.device.device_id,
                    session_id: s.session_id,
                    household_id: s.device.household_id,
                    client_addr,
                    request_id,
                })
            }
            _ => Err(ApiError::new(crate::error::Problem::Unauthenticated)),
        }
    }
}

/// Query deserialization must use the same RFC problem envelope as body parsing.
pub struct ApiQuery<T>(pub T);
impl<T: DeserializeOwned + Send> FromRequestParts<AppState> for ApiQuery<T> {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let axum::extract::Query(value) =
            axum::extract::Query::<T>::from_request_parts(parts, state)
                .await
                .map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
        Ok(Self(value))
    }
}

#[cfg(all(test, feature = "testing"))]
mod mutation_metadata_tests {
    use super::*;
    use tower::ServiceExt;
    #[sqlx::test(migrations = "../db/migrations")]
    async fn generic_mutation_accepts_non_uuid_target(pool: sqlx::PgPool) {
        let s = crate::testing::TestServer::start(&pool).await;
        async fn probe(m: Mutation<serde_json::Value>) -> axum::http::StatusCode {
            assert!(m.context.target_id.is_none());
            axum::http::StatusCode::OK
        }
        let app = axum::Router::new()
            .route("/probe/{id}", axum::routing::post(probe))
            .with_state(s.state.clone());
        let mut req = Request::builder()
            .method("POST")
            .uri("/probe/not-a-uuid")
            .header("content-type", "application/json")
            .header("idempotency-key", "probe")
            .body(axum::body::Body::from("{}"))
            .unwrap();
        req.extensions_mut().insert(MutationAuth::Enrollment);
        assert_eq!(
            app.oneshot(req).await.unwrap().status(),
            axum::http::StatusCode::OK
        );
        s.stop().await;
    }
}
