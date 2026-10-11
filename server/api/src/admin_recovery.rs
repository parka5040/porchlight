//! Owner-run recovery through the local server credential, never through a listener.
use anyhow::{Context, ensure};
use chrono::{DateTime, Duration, Utc};
use porchlight_core::auth::token::{Token, TokenKind};
use porchlight_db::{
    tokens,
    tx::{self, Actor},
};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn mint(pool: &PgPool, member: &str, name: &str, days: u32) -> anyhow::Result<Token> {
    ensure!(
        crate::routes::admin::valid_label(name),
        "name must contain 1 to 60 characters"
    );
    ensure!(days > 0, "expires-days must be positive");
    let expires_at = Duration::try_days(i64::from(days))
        .and_then(|duration| Utc::now().checked_add_signed(duration))
        .context("invalid token lifetime")?;
    let id = Uuid::now_v7();
    // The token id identifies the issued credential, not a pre-existing authorization.
    let actor = Actor {
        agent_label: Some("local-recovery".into()),
        token_id: Some(id),
        ..Actor::system()
    };
    let mut tx = tx::begin_as(pool, &actor)
        .await
        .map_err(crate::db::database_error)?;
    let owner = tokens::recovery_owner(&mut tx, member.parse().ok(), member)
        .await
        .map_err(crate::db::database_error)?
        .context("initialized household and live super-admin member required (UUID or username)")?;
    let token = Token::generate(TokenKind::Pat);
    let owner = tokens::PatIdentity {
        token_id: id,
        household_id: owner.household_id,
        member_id: owner.member_id,
        user_id: owner.user_id,
        username: owner.username,
        name: name.into(),
        scopes: vec!["admin".into()],
        expires_at,
        last_used_at: None,
    };
    tokens::create_with_id(
        &mut tx,
        id,
        &owner,
        name,
        &owner.scopes,
        &token.hash(),
        expires_at,
    )
    .await
    .map_err(crate::db::database_error)?;
    porchlight_db::security_events::record(
        &mut tx,
        owner.household_id,
        porchlight_db::security_events::Kind::AdminTokenRecovered,
        id,
    )
    .await
    .map_err(crate::db::database_error)?;
    tx.commit().await.map_err(crate::db::database_error)?;
    Ok(token)
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct AdminTokenHealth {
    pub newest_live_expires_at: Option<DateTime<Utc>>,
    pub expiring_soon: bool,
    pub recovery_required: bool,
}
pub async fn health(pool: &PgPool) -> sqlx::Result<AdminTokenHealth> {
    let expires_at = tokens::newest_admin_expiry(pool).await?;
    Ok(AdminTokenHealth {
        newest_live_expires_at: expires_at,
        expiring_soon: expires_at.is_some_and(|at| at <= Utc::now() + Duration::days(30)),
        recovery_required: expires_at.is_none(),
    })
}
pub async fn warn(pool: &PgPool) {
    match health(pool).await {
        Ok(health) if health.recovery_required => tracing::warn!(
            reason = "admin-token-missing",
            "no live admin PAT; use owner-run local recovery"
        ),
        Ok(health) if health.expiring_soon => tracing::warn!(
            reason = "admin-token-expiring",
            "newest live admin PAT expires within 30 days; rotate it"
        ),
        Ok(_) => {}
        Err(_) => tracing::warn!(
            reason = "admin-token-health-unavailable",
            "admin PAT expiry check failed"
        ),
    }
}
