//! Secret-free security history. No request bodies or free-form event payloads enter this API.
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub enum Kind {
    LoginFailed,
    LoginSucceeded,
    PasswordChanged,
    PasswordResetRedeemed,
    CertificateRenewed,
    AdminTokenRecovered,
}
impl Kind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::LoginFailed => "login_failed",
            Self::LoginSucceeded => "login_succeeded",
            Self::PasswordChanged => "password_changed",
            Self::PasswordResetRedeemed => "password_reset_redeemed",
            Self::CertificateRenewed => "certificate_renewed",
            Self::AdminTokenRecovered => "admin_token_recovered",
        }
    }
}

pub async fn record(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    kind: Kind,
    target: Uuid,
) -> sqlx::Result<()> {
    let target_type = match kind {
        Kind::CertificateRenewed => "device",
        Kind::AdminTokenRecovered => "token",
        _ => "member",
    };
    let outcome = if matches!(kind, Kind::LoginFailed) {
        "failed"
    } else {
        "succeeded"
    };
    sqlx::query("INSERT INTO porchlight.security_events(household_id,kind,target_type,target_id,outcome) VALUES($1,$2,$3,$4,$5)")
        .bind(household).bind(kind.as_str()).bind(target_type).bind(target).bind(outcome)
        .execute(&mut **tx).await?;
    Ok(())
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Event {
    pub id: Uuid,
    pub household_id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub kind: String,
    pub actor: serde_json::Value,
    pub target_type: String,
    pub target_id: Uuid,
    pub outcome: String,
}

pub async fn list(
    pool: &PgPool,
    household: Uuid,
    since: Option<DateTime<Utc>>,
    kind: Option<&str>,
    after: Option<(DateTime<Utc>, Uuid)>,
    limit: i64,
) -> sqlx::Result<Vec<Event>> {
    let (time, id) = after.map(|(t, i)| (Some(t), Some(i))).unwrap_or_default();
    sqlx::query_as("SELECT id,household_id,occurred_at,kind,actor,target_type,target_id,outcome FROM porchlight.security_events WHERE household_id=$1 AND ($2::timestamptz IS NULL OR occurred_at>=$2) AND ($3::text IS NULL OR kind=$3) AND ($4::timestamptz IS NULL OR (occurred_at,id)>($4,$5)) ORDER BY occurred_at,id LIMIT $6")
        .bind(household).bind(since).bind(kind).bind(time).bind(id).bind(limit).fetch_all(pool).await
}
