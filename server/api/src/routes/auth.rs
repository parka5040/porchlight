//! Device-bound authentication. Logout is terminal: retries authenticate first and receive session-invalid.
use crate::{
    error::ApiError,
    extract::{DeviceAuth, Mutation, SessionAuth, actor},
    idempotency::{self, Begin, IdemScope, IdemTx},
    ratelimit::Class,
    server::ConnInfo,
    state::AppState,
};
use axum::{
    Extension, Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::DateTime;
use chrono::{Duration, Utc};
use porchlight_core::auth::{
    lockout::locked_until,
    password::{PolicyError, check_policy},
    reset_code::hash_reset_code,
    session::{Platform, STEP_UP_MINUTES, StepUpAction, session_lifetime},
    token::{Token, TokenKind, hashes_equal},
};
use porchlight_db::{
    auth::{self, DeviceIdentity},
    idempotency::ScopeKind,
};
use serde::Deserialize;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Login {
    password: String,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Logout {}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Password {
    current_password: String,
    new_password: String,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Reauth {
    password: String,
    action: String,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetPassword {
    code: String,
    new_password: String,
}

async fn start<T>(
    state: &AppState,
    d: &DeviceIdentity,
    info: &ConnInfo,
    m: &Mutation<T>,
    request_id: uuid::Uuid,
    class: Class,
) -> Result<Begin, ApiError> {
    state
        .limits
        .check(class, Some(d.device_id), info.client_addr.map(|a| a.ip()))?;
    idempotency::begin(
        state,
        &actor(d, request_id),
        &m.auth,
        IdemScope {
            kind: ScopeKind::Device,
            scope_id: d.device_id,
            household_id: d.household_id,
        },
        &m.key,
        &m.request_hash,
    )
    .await
}
pub(super) fn policy(password: &str) -> Result<(), ApiError> {
    check_policy(password).map_err(|e| {
        ApiError::new(crate::error::Problem::PasswordPolicy).detail(match e {
            PolicyError::TooShort => "too_short",
            PolicyError::TooLong => "too_long",
            PolicyError::Breached => "breached",
        })
    })
}
/// Own the permit inside the blocking job, so cancellation cannot release a still-running hash's slot.
async fn verify(state: &AppState, password: &str, phc: Option<String>) -> Result<bool, ApiError> {
    let permit = state
        .hash_slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("password_semaphore_closed"))?;
    let hasher = state.hasher.clone();
    let password = zeroize::Zeroizing::new(password.to_owned());
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if let Some(phc) = phc {
            hasher.verify(&password, &phc)
        } else {
            hasher.verify_dummy(&password);
            false
        }
    })
    .await
    .map_err(|e| {
        ApiError::internal(if e.is_panic() {
            "password_job_panicked"
        } else {
            "password_job_cancelled"
        })
    })
}
pub(super) async fn hash(state: &AppState, password: &str) -> Result<String, ApiError> {
    let permit = state
        .hash_slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("password_semaphore_closed"))?;
    let hasher = state.hasher.clone();
    let password = zeroize::Zeroizing::new(password.to_owned());
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hasher.hash(&password)
    })
    .await
    .map_err(|e| {
        ApiError::internal(if e.is_panic() {
            "password_job_panicked"
        } else {
            "password_job_cancelled"
        })
    })
}
/// Serialize failures and lock checks; replay callers never get this far.
async fn check_password(
    state: &AppState,
    tx: &mut IdemTx,
    d: &DeviceIdentity,
    password: &str,
    new: Option<&str>,
) -> Result<(), ApiError> {
    check_user_password(state, tx, d.user_id, password, new).await
}
/// Shared lockout and bounded password verification for enrollment and PAT issuance.
pub(crate) async fn check_user_password(
    state: &AppState,
    tx: &mut IdemTx,
    user_id: uuid::Uuid,
    password: &str,
    new: Option<&str>,
) -> Result<(), ApiError> {
    let credentials = auth::lock_credentials(&mut tx.tx, user_id).await?;
    tx.recheck_session().await?;
    let now = Utc::now();
    if let Some(ref c) = credentials
        && let Some(until) = c.locked_until.filter(|until| *until > now)
    {
        let remaining = until - now;
        let whole = remaining.num_seconds();
        let seconds = whole + i64::from(remaining > Duration::seconds(whole));
        return Err(
            ApiError::new(crate::error::Problem::AccountLocked).retry(seconds.max(1) as u64)
        );
    }
    if let Some(new) = new {
        policy(new)?;
    }
    let phc = credentials.as_ref().map(|c| c.password_hash.clone());
    if !verify(state, password, phc).await? {
        if let Some(c) = credentials {
            let n = c.failed_attempts.saturating_add(1);
            auth::record_failure(&mut tx.tx, user_id, n, locked_until(n, Utc::now())).await?;
        }
        return Err(ApiError::new(crate::error::Problem::InvalidCredentials));
    }
    auth::record_success(&mut tx.tx, user_id).await?;
    Ok(())
}
/// Caller has already locked and checked the live device before revoking old sessions.
async fn session(tx: &mut IdemTx, d: &DeviceIdentity) -> Result<Response, ApiError> {
    let platform = Platform::parse(&d.platform)
        .ok_or_else(|| ApiError::internal("auth_invalid_stored_data"))?;
    let token = Token::generate(TokenKind::Session);
    let expires_at = Utc::now() + session_lifetime(platform);
    auth::create_session(
        &mut tx.tx,
        d.household_id,
        d.device_id,
        &token.hash(),
        expires_at,
    )
    .await?;
    Ok(Json(SessionResponse {
        session: SessionToken {
            token: token.expose().to_owned(),
            expires_at,
        },
    })
    .into_response())
}
#[utoipa::path(post,
    path="/v1/auth/login",
    request_body=Login,
    responses((status=200,description="Success",body=crate::routes::auth::SessionResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem))]
pub async fn login(
    State(state): State<AppState>,
    DeviceAuth(d, request_id): DeviceAuth,
    Extension(info): Extension<ConnInfo>,
    m: Mutation<Login>,
) -> Result<Response, ApiError> {
    let begin = start(&state, &d, &info, &m, request_id, Class::Auth).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        if let Err(e) = check_password(&state, &mut tx, &d, &m.body.password, None).await {
            if e.is_transient() {
                return Err(e);
            }
            return Ok((tx, e.into_response()));
        }
        // Acquire the device lock before revoking earlier sessions or issuing a replacement.
        if !auth::lock_live_device(&mut tx.tx, d.device_id).await? {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::DeviceRevoked).into_response(),
            ));
        }
        auth::revoke_device_sessions(&mut *tx.tx, d.device_id).await?;
        let r = match session(&mut tx, &d).await {
            Ok(r) => r,
            Err(e) if e.is_transient() => return Err(e),
            Err(e) => e.into_response(),
        };
        Ok((tx, r))
    })
    .await
}
#[utoipa::path(post,
    path="/v1/auth/logout",
    request_body=Logout,
    responses((status=204,description="Success"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem),
    security(("session" = [])))]
pub async fn logout(
    State(state): State<AppState>,
    s: SessionAuth,
    Extension(info): Extension<ConnInfo>,
    m: Mutation<Logout>,
) -> Result<Response, ApiError> {
    let begin = start(&state, &s.device, &info, &m, s.request_id, Class::Write).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        auth::revoke_session(&mut *tx.tx, s.session_id).await?;
        Ok((tx, StatusCode::NO_CONTENT.into_response()))
    })
    .await
}
#[utoipa::path(post,
    path="/v1/auth/password",
    request_body=Password,
    responses((status=204,description="Success"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem),
    security(("session" = [])))]
pub async fn password(
    State(state): State<AppState>,
    s: SessionAuth,
    Extension(info): Extension<ConnInfo>,
    m: Mutation<Password>,
) -> Result<Response, ApiError> {
    let begin = start(&state, &s.device, &info, &m, s.request_id, Class::Auth).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        if let Err(e) = check_password(
            &state,
            &mut tx,
            &s.device,
            &m.body.current_password,
            Some(&m.body.new_password),
        )
        .await
        {
            if e.is_transient() {
                return Err(e);
            }
            return Ok((tx, e.into_response()));
        }
        let phc = hash(&state, &m.body.new_password).await?;
        auth::set_password(&mut tx.tx, s.device.user_id, &phc).await?;
        auth::revoke_user_sessions(&mut *tx.tx, s.device.user_id, Some(s.session_id)).await?;
        Ok((tx, StatusCode::NO_CONTENT.into_response()))
    })
    .await
}
#[utoipa::path(post,
    path="/v1/auth/reauth",
    request_body=Reauth,
    responses((status=200,description="Success",body=crate::routes::auth::ReauthResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem),
    security(("session" = [])))]
pub async fn reauth(
    State(state): State<AppState>,
    s: SessionAuth,
    Extension(info): Extension<ConnInfo>,
    m: Mutation<Reauth>,
) -> Result<Response, ApiError> {
    let begin = start(&state, &s.device, &info, &m, s.request_id, Class::Auth).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        let Some(action) = StepUpAction::parse(&m.body.action) else {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::InvalidRequest).into_response(),
            ));
        };
        if let Err(e) = check_password(&state, &mut tx, &s.device, &m.body.password, None).await {
            if e.is_transient() {
                return Err(e);
            }
            return Ok((tx, e.into_response()));
        }
        let token = Token::generate(TokenKind::StepUp);
        let expires_at = Utc::now() + Duration::minutes(STEP_UP_MINUTES);
        auth::create_step_up(
            &mut tx.tx,
            s.device.household_id,
            s.session_id,
            s.device.device_id,
            action.as_str(),
            &token.hash(),
            expires_at,
        )
        .await?;
        Ok((
            tx,
            Json(ReauthResponse {
                step_up_token: token.expose().to_owned(),
                expires_at,
            })
            .into_response(),
        ))
    })
    .await
}
#[utoipa::path(post,
    path="/v1/auth/reset-password",
    request_body=ResetPassword,
    responses((status=200,description="Success",body=crate::routes::auth::SessionResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem))]
pub async fn reset_password(
    State(state): State<AppState>,
    DeviceAuth(d, request_id): DeviceAuth,
    Extension(info): Extension<ConnInfo>,
    m: Mutation<ResetPassword>,
) -> Result<Response, ApiError> {
    let begin = start(&state, &d, &info, &m, request_id, Class::Auth).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        // Reset is the escape from password lockout, but shares its credentials-row serialization.
        let credentials = auth::lock_credentials(&mut tx.tx, d.user_id).await?;
        if credentials.is_none() {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::ResetCodeInvalid).into_response(),
            ));
        }
        if let Err(e) = policy(&m.body.new_password) {
            if e.is_transient() {
                return Err(e);
            }
            return Ok((tx, e.into_response()));
        }
        let Some(reset) = auth::lock_active_reset(&mut tx.tx, d.user_id).await? else {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::ResetCodeInvalid).into_response(),
            ));
        };
        let stored: [u8; 32] = reset
            .code_hash
            .try_into()
            .map_err(|_| ApiError::internal("auth_invalid_stored_data"))?;
        if !hash_reset_code(&m.body.code).is_some_and(|h| hashes_equal(&h, &stored)) {
            auth::record_reset_failure(&mut tx.tx, reset.id).await?;
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::ResetCodeInvalid).into_response(),
            ));
        }
        if !auth::lock_live_device(&mut tx.tx, d.device_id).await? {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::DeviceRevoked).into_response(),
            ));
        }
        let phc = hash(&state, &m.body.new_password).await?;
        auth::set_password(&mut tx.tx, d.user_id, &phc).await?;
        auth::consume_reset(&mut tx.tx, reset.id).await?;
        auth::revoke_user_sessions(&mut *tx.tx, d.user_id, None).await?;
        let r = match session(&mut tx, &d).await {
            Ok(r) => r,
            Err(e) if e.is_transient() => return Err(e),
            Err(e) => e.into_response(),
        };
        Ok((tx, r))
    })
    .await
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(login, logout, password, reauth, reset_password))]
pub struct AuthApi;

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct SessionToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct SessionResponse {
    pub session: SessionToken,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ReauthResponse {
    pub step_up_token: String,
    pub expires_at: DateTime<Utc>,
}
