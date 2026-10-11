//! LAN-only enrollment. Token authentication precedes replay; used/expired tokens can replay their sealed success.
use crate::{
    error::ApiError,
    extract::Mutation,
    idempotency::{self, IdemScope},
    state::AppState,
};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use porchlight_core::auth::{
    session::{Platform, session_lifetime},
    token::{Token, TokenKind, hash_token, looks_like},
};
use porchlight_db::{auth, devices, enrollment, idempotency::ScopeKind, tx::Actor};
use serde::Deserialize;
use uuid::Uuid;
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Complete {
    token: String,
    password: String,
    csr: String,
    platform: String,
}
#[utoipa::path(post,
    path="/v1/enrollment/complete",
    request_body=Complete,
    responses((status=201,description="Success",body=EnrollmentResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,(status=410,description="Enrollment token used or expired",body=crate::openapi::ProblemBody,content_type="application/problem+json")))]
pub async fn complete(
    State(state): State<AppState>,
    m: Mutation<Complete>,
) -> Result<Response, ApiError> {
    if !looks_like(TokenKind::Enrollment, &m.body.token) {
        return Err(ApiError::new(crate::error::Problem::EnrollmentTokenInvalid));
    }
    let token = enrollment::find(&state.pool, &hash_token(&m.body.token))
        .await?
        .ok_or_else(|| ApiError::new(crate::error::Problem::EnrollmentTokenInvalid))?;
    let begin = idempotency::begin(
        &state,
        &Actor {
            member_id: Some(token.member_id),
            device_id: None,
            request_id: m.request_id,
            agent_label: None,
            token_id: None,
        },
        &m.auth,
        IdemScope {
            kind: ScopeKind::Enrollment,
            scope_id: token.id,
            household_id: token.household_id,
        },
        &m.key,
        &m.request_hash,
    )
    .await?;
    crate::mutation::execute(&state, begin, |mut tx| async {
        let token = enrollment::lock(&mut tx.tx, token.id)
            .await
            .map_err(|error| {
                // Retention may delete an expired token after the initial lookup.
                // Treat it like a missing token and roll back the unsealed claim.
                if matches!(error, sqlx::Error::RowNotFound) {
                    ApiError::new(crate::error::Problem::EnrollmentTokenInvalid)
                } else {
                    ApiError::from(error)
                }
            })?;
        let error = if token.used_at.is_some() {
            Some(crate::error::Problem::EnrollmentTokenUsed)
        } else if token.expires_at <= Utc::now() {
            Some(crate::error::Problem::EnrollmentTokenExpired)
        } else {
            None
        };
        if let Some(slug) = error {
            return Ok((tx, ApiError::new(slug).into_response()));
        }
        let Some(member) = enrollment::live_member(&mut *tx.tx, token.member_id).await? else {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::Unauthenticated).into_response(),
            ));
        };
        if let Err(e) = super::auth::check_user_password(
            &state,
            &mut tx,
            member.user_id,
            &m.body.password,
            None,
        )
        .await
        {
            if e.is_transient() {
                return Err(e);
            }
            return Ok((tx, e.into_response()));
        }
        let Some(platform) = Platform::parse(&m.body.platform) else {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::InvalidRequest).into_response(),
            ));
        };
        if porchlight_pki::validate_csr(&m.body.csr).is_err() {
            return Ok((
                tx,
                ApiError::new(crate::error::Problem::CsrInvalid).into_response(),
            ));
        }
        let id = Uuid::now_v7();
        let cert = state
            .pki
            .ca
            .issue_device(
                &m.body.csr,
                &id.to_string(),
                std::time::SystemTime::now(),
                state.settings.device_cert_days,
            )
            .map_err(|_| ApiError::internal("enrollment_certificate_issue"))?;
        devices::create(
            &mut tx.tx,
            id,
            token.household_id,
            token.member_id,
            &token.device_label,
            platform.as_str(),
        )
        .await?;
        super::devices::store_certificate(&mut tx.tx, token.household_id, id, &cert).await?;
        let session = Token::generate(TokenKind::Session);
        let expires_at = Utc::now() + session_lifetime(platform);
        auth::create_session(
            &mut tx.tx,
            token.household_id,
            id,
            &session.hash(),
            expires_at,
        )
        .await?;
        enrollment::consume(&mut tx.tx, token.id).await?;
        Ok((
            tx,
            (
                StatusCode::CREATED,
                Json(EnrollmentResponse {
                    device_id: id,
                    certificate: cert.cert_pem,
                    ca_certificate: state.pki.ca.cert().cert_pem.clone(),
                    session: super::auth::SessionToken {
                        token: session.expose().to_owned(),
                        expires_at,
                    },
                    endpoints: state.settings.endpoints.clone(),
                }),
            )
                .into_response(),
        ))
    })
    .await
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(complete))]
pub struct EnrollmentApi;

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct EnrollmentResponse {
    pub device_id: Uuid,
    pub certificate: String,
    pub ca_certificate: String,
    pub session: super::auth::SessionToken,
    pub endpoints: Vec<String>,
}
