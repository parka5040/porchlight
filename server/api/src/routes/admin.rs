//! Agent-only administration. Any PAT may call whoami; every other route needs admin scope.
//! Revoking the calling PAT is terminal: retry authentication returns 401, never a replay.
use super::devices::Empty;
use crate::{
    error::ApiError,
    extract::{AdminAuth, Mutation, PatAuth},
    idempotency::{self, Begin, IdemScope},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::DateTime;
use chrono::{Duration, Utc};
use porchlight_core::auth::{
    enroll_uri::EnrollUri,
    reset_code::{ResetCode, hash_reset_code},
    session::{ENROLLMENT_TOKEN_MINUTES, RESET_CODE_MINUTES},
    token::{Token, TokenKind},
};
use porchlight_db::{auth, devices, enrollment, idempotency::ScopeKind, tokens};
use serde::Deserialize;
use uuid::Uuid;
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentToken {
    username: String,
    device_label: String,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateToken {
    name: String,
    scopes: Vec<String>,
    #[serde(default = "default_days")]
    expires_days: u32,
    password: Option<String>,
}
fn default_days() -> u32 {
    365
}
pub(crate) fn valid_label(label: &str) -> bool {
    (1..=60).contains(&label.trim().chars().count()) && label.chars().count() <= 60
}
async fn start<T>(
    state: &AppState,
    p: &PatAuth,
    m: &Mutation<T>,
    revoke_target: Option<Uuid>,
) -> Result<Begin, ApiError> {
    // AdminAuth owns the scope check; registered self-PAT-revoke is terminal.
    let scope = IdemScope {
        kind: ScopeKind::Pat,
        scope_id: p.0.token_id,
        household_id: p.0.household_id,
    };
    match revoke_target {
        Some(target) => {
            idempotency::begin_pat_revoke(
                state,
                &p.actor(),
                &m.auth,
                scope,
                &m.key,
                &m.request_hash,
                target,
            )
            .await
        }
        None => {
            idempotency::begin(state, &p.actor(), &m.auth, scope, &m.key, &m.request_hash).await
        }
    }
}
#[utoipa::path(get,
    path="/v1/admin/whoami",
    responses((status=200,description="Success",body=WhoamiResponse),crate::openapi::UniversalProblems),
    security(("pat" = [])))]
pub async fn whoami(p: PatAuth) -> Json<WhoamiResponse> {
    Json(WhoamiResponse {
        token_id: p.0.token_id,
        name: p.0.name,
        scopes: p.0.scopes,
        member_id: p.0.member_id,
        username: p.0.username,
        expires_at: p.0.expires_at,
    })
}
#[utoipa::path(post,
    path="/v1/admin/enrollment-tokens",
    request_body=EnrollmentToken,
    responses((status=201,description="Success",body=EnrollmentTokenResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem),
    security(("pat" = [])))]
pub async fn enrollment_tokens(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    m: Mutation<EnrollmentToken>,
) -> Result<Response, ApiError> {
    let begin = start(&state, &p, &m, None).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        if !valid_label(&m.body.device_label) {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::InvalidRequest).into_response(),
            ));
        }
        let Some(member) =
            enrollment::by_username(&mut *tx.tx, p.0.household_id, &m.body.username).await?
        else {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::NotFound).into_response(),
            ));
        };
        let token = Token::generate(TokenKind::Enrollment);
        let expires_at = Utc::now() + Duration::minutes(ENROLLMENT_TOKEN_MINUTES);
        enrollment::create(
            &mut tx.tx,
            &member,
            &m.body.device_label,
            &token.hash(),
            p.0.member_id,
            expires_at,
        )
        .await?;
        let fp = state.pki.ca.cert().fingerprint;
        let uri = EnrollUri {
            host: state.settings.enroll_host.clone(),
            token: token.expose().to_owned(),
            ca_fingerprint: fp,
        }
        .to_uri();
        Ok((
            tx,
            (
                StatusCode::CREATED,
                Json(EnrollmentTokenResponse {
                    token: token.expose().to_owned(),
                    uri,
                    expires_at,
                    ca_fingerprint: porchlight_pki::format_fingerprint(&fp),
                }),
            )
                .into_response(),
        ))
    })
    .await
}
#[utoipa::path(post,
    path="/v1/admin/users/{username}/password-reset",
    request_body=crate::routes::devices::Empty,
    params(("username" = String, Path, description = "Resource identifier")),
    responses((status=201,description="Success",body=PasswordResetResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem),
    security(("pat" = [])))]
pub async fn password_reset(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(username): Path<String>,
    m: Mutation<Empty>,
) -> Result<Response, ApiError> {
    let begin = start(&state, &p, &m, None).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        let Some(member) =
            enrollment::by_username(&mut *tx.tx, p.0.household_id, &username).await?
        else {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::NotFound).into_response(),
            ));
        };
        // Serialize issuance with redemption and other issuers, so only the newest code remains usable.
        if auth::lock_credentials(&mut tx.tx, member.user_id)
            .await?
            .is_none()
        {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::NotFound).into_response(),
            ));
        }
        enrollment::void_resets(&mut tx.tx, member.user_id).await?;
        let code = ResetCode::generate();
        let expires_at = Utc::now() + Duration::minutes(RESET_CODE_MINUTES);
        auth::create_password_reset(
            &mut tx.tx,
            member.household_id,
            member.user_id,
            &hash_reset_code(code.expose()).expect("generated code"),
            p.0.member_id,
            expires_at,
        )
        .await?;
        Ok((
            tx,
            (
                StatusCode::CREATED,
                Json(PasswordResetResponse {
                    code: code.expose().to_owned(),
                    expires_at,
                }),
            )
                .into_response(),
        ))
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDevicesParams {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
    pub member_id: Option<Uuid>,
}
#[utoipa::path(get,
    path="/v1/admin/devices",
    params(("member_id"=Option<Uuid>, Query,description="Filter by household member"),("limit" = Option<u16>, Query, description = "Default 50; maximum 200"),("cursor" = Option<String>, Query, description = "Opaque signed keyset cursor")),
    responses((status=200,description="Success",body=crate::routes::devices::DevicesResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem),
    security(("pat" = [])))]
pub async fn list_devices(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    page: Result<
        axum::extract::Query<AdminDevicesParams>,
        axum::extract::rejection::QueryRejection,
    >,
) -> Result<Json<super::devices::DevicesResponse>, ApiError> {
    let axum::extract::Query(page) =
        page.map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
    let limit = crate::pagination::PageParams {
        cursor: None,
        limit: page.limit,
    }
    .limit()?;
    p.require_admin()?;
    let context = format!("admin-devices:{}:{:?}", p.0.household_id, page.member_id);
    let after = page
        .cursor
        .as_deref()
        .map(|c| state.idem.cursor.decode(&context, c))
        .transpose()?;
    let rows = devices::list_page(
        &state.pool,
        p.0.household_id,
        page.member_id,
        None,
        true,
        after,
        (limit + 1) as i64,
    )
    .await?;
    let page = state
        .idem
        .cursor
        .page(&context, rows, limit, |r| (r.created_at, r.id))?;
    Ok(Json(super::devices::DevicesResponse {
        devices: page.items.iter().cloned().map(Into::into).collect(),
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next_cursor,
    }))
}
#[utoipa::path(post,
    path="/v1/admin/devices/{id}/revoke",
    request_body=crate::routes::devices::Empty,
    params(("id" = String, Path, description = "Resource identifier")),
    responses((status=200,description="Success",body=crate::routes::devices::RevocationResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem),
    security(("pat" = [])))]
pub async fn revoke_device(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(id): Path<String>,
    m: Mutation<Empty>,
) -> Result<Response, ApiError> {
    let id =
        Uuid::parse_str(&id).map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
    let begin = start(&state, &p, &m, None).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        let Some(revocation) = devices::revoke(&mut tx.tx, p.0.household_id, None, id).await?
        else {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::NotFound).into_response(),
            ));
        };
        tx.publish_revocation(id, revocation.fingerprints);
        Ok((
            tx,
            Json(super::devices::RevocationResponse {
                revoked_at: revocation.revoked_at,
            })
            .into_response(),
        ))
    })
    .await
}
#[utoipa::path(post,
    path="/v1/admin/tokens",
    request_body=CreateToken,
    responses((status=201,description="Success",body=TokenResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem),
    security(("pat" = [])))]
pub async fn create_token(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    m: Mutation<CreateToken>,
) -> Result<Response, ApiError> {
    p.require_admin()?;
    let admin = m.body.scopes.iter().any(|s| s == "admin");
    let begin = start(&state, &p, &m, None).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        let expires_at = Duration::try_days(i64::from(m.body.expires_days))
            .and_then(|d| Utc::now().checked_add_signed(d));
        if !valid_label(&m.body.name)
            || m.body.scopes.is_empty()
            || m.body
                .scopes
                .iter()
                .any(|s| !matches!(s.as_str(), "read" | "write" | "admin"))
            || m.body.expires_days == 0
            || expires_at.is_none()
        {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::InvalidRequest).into_response(),
            ));
        }
        if admin && m.body.password.as_deref().is_none_or(str::is_empty) {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::InvalidCredentials).into_response(),
            ));
        }
        if admin
            && let Err(e) = super::auth::check_user_password(
                &state,
                &mut tx,
                p.0.user_id,
                m.body.password.as_deref().unwrap_or(""),
                None,
            )
            .await
        {
            if e.is_transient() {
                return Err(e);
            }
            return Ok((tx, e.into_response()));
        }
        let token = Token::generate(TokenKind::Pat);
        let expires_at = expires_at.expect("validated lifetime");
        let id = tokens::create_with_id(
            &mut tx.tx,
            Uuid::now_v7(),
            &p.0,
            &m.body.name,
            &m.body.scopes,
            &token.hash(),
            expires_at,
        )
        .await?;
        Ok((
            tx,
            (
                StatusCode::CREATED,
                Json(TokenResponse {
                    id,
                    token: token.expose().to_owned(),
                    scopes: m.body.scopes,
                    expires_at,
                }),
            )
                .into_response(),
        ))
    })
    .await
}
#[utoipa::path(post,
    path="/v1/admin/tokens/{id}/revoke",
    request_body=crate::routes::devices::Empty,
    params(("id" = String, Path, description = "Resource identifier")),
    responses((status=200,description="Success",body=crate::routes::devices::RevocationResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem),
    security(("pat" = [])))]
pub async fn revoke_token(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    Path(id): Path<String>,
    m: Mutation<Empty>,
) -> Result<Response, ApiError> {
    let id =
        Uuid::parse_str(&id).map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
    let begin = start(&state, &p, &m, Some(id)).await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        let Some(revoked_at) = tokens::revoke(&mut tx.tx, p.0.household_id, id).await? else {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::NotFound).into_response(),
            ));
        };
        Ok((
            tx,
            Json(super::devices::RevocationResponse { revoked_at }).into_response(),
        ))
    })
    .await
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ListedToken {
    pub id: Uuid,
    pub owner_member_id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct TokenList {
    pub tokens: Vec<ListedToken>,
    pub admin_token_health: crate::admin_recovery::AdminTokenHealth,
}
#[utoipa::path(get, path="/v1/admin/tokens", security(("pat"=[])),
    responses((status=200,description="Token metadata (never credentials) and newest live admin PAT expiry health",body=TokenList),crate::openapi::ForbiddenProblem,crate::openapi::UniversalProblems))]
pub async fn list_tokens(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
) -> Result<Json<TokenList>, ApiError> {
    let tokens = tokens::list(&state.pool, p.0.household_id)
        .await?
        .into_iter()
        .map(|t| ListedToken {
            id: t.id,
            owner_member_id: t.owner_member_id,
            name: t.name,
            scopes: t.scopes,
            created_at: t.created_at,
            expires_at: t.expires_at,
            last_used_at: t.last_used_at,
            revoked_at: t.revoked_at,
        })
        .collect();
    Ok(Json(TokenList {
        tokens,
        admin_token_health: crate::admin_recovery::health(&state.pool).await?,
    }))
}
fn token_list_policy(c: &crate::permissions::PolicyContext) -> u16 {
    match c.pat_scope {
        Some(crate::permissions::PatScope::Admin) => 200,
        Some(_) => 403,
        None => 404,
    }
}
pub const ENDPOINTS: &[crate::permissions::Endpoint] = &[crate::permissions::Endpoint {
    method: "GET",
    path: "/v1/admin/tokens",
    handler: "list_tokens",
    policy: crate::permissions::Policy::Custom {
        label: "Admin PAT metadata and expiry health",
        expected: token_list_policy,
    },
    case: "",
}];

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    whoami,
    list_tokens,
    enrollment_tokens,
    password_reset,
    list_devices,
    revoke_device,
    create_token,
    revoke_token
))]
pub struct AdminApi;

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct WhoamiResponse {
    pub token_id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
    pub member_id: Uuid,
    pub username: String,
    pub expires_at: DateTime<Utc>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct EnrollmentTokenResponse {
    pub token: String,
    pub uri: String,
    pub expires_at: DateTime<Utc>,
    pub ca_fingerprint: String,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct PasswordResetResponse {
    pub code: String,
    pub expires_at: DateTime<Utc>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct TokenResponse {
    pub id: Uuid,
    pub token: String,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
}
