//! Serialized, recoverable first installation. Only staged output contains the plaintext PAT.
use crate::pki_store::{
    DataDir, normalize_server_sans, private_dir, read_secret_text, sync_dir, write_secret_file,
};
use anyhow::{Context, bail, ensure};
use chrono::{DateTime, Utc};
use porchlight_core::auth::{
    password::{PasswordHasher, check_policy},
    token::{Token, TokenKind},
};
use porchlight_pki::{HouseholdCa, ServerSans, format_fingerprint, parse_cert_pem};
use sqlx::{PgPool, Postgres, Transaction};
use std::{
    fmt,
    fs::{self, File},
    io,
    time::{Duration, SystemTime},
};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
#[error("already initialized")]
pub struct AlreadyInitialized;
pub struct InitParams {
    pub household_name: String,
    pub tzid: String,
    pub admin_username: String,
    pub admin_display_name: String,
    pub admin_password: Zeroizing<String>,
    pub sans: ServerSans,
    pub age_recipient: String,
}
impl fmt::Debug for InitParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InitParams")
            .field("admin_password", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct InitOutput {
    pub household_id: Uuid,
    pub admin_member_id: Uuid,
    pub ca_fingerprint: String,
    pub bootstrap_token: String,
    pub data_dir: std::path::PathBuf,
}
impl fmt::Debug for InitOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InitOutput")
            .field("household_id", &self.household_id)
            .field("ca_fingerprint", &self.ca_fingerprint)
            .field("bootstrap_token", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
fn validate(p: &mut InitParams) -> anyhow::Result<()> {
    check_policy(&p.admin_password).context("admin password rejected")?;
    let u = p.admin_username.as_bytes();
    ensure!(
        (2..=32).contains(&u.len())
            && u[0].is_ascii_alphanumeric()
            && u.iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(b)),
        "invalid admin username"
    );
    let recipient = p.age_recipient.as_bytes();
    ensure!(
        recipient.len() == 62
            && recipient.starts_with(b"age1")
            && recipient[4..]
                .iter()
                .all(|b| b"023456789acdefghjklmnpqrstuvwxyz".contains(b)),
        "invalid age recipient"
    );
    ensure!(
        (1..=100).contains(&p.household_name.trim().chars().count()),
        "invalid household name"
    );
    ensure!(
        (1..=60).contains(&p.admin_display_name.trim().chars().count()),
        "invalid admin display name"
    );
    normalize_server_sans(&mut p.sans)
}
fn db_error(e: sqlx::Error) -> anyhow::Error {
    let already = e.as_database_error().is_some_and(|e| {
        e.code().as_deref() == Some("23505") && e.constraint() == Some("install_state_pkey")
    });
    let detail = crate::db::database_error(e);
    if already {
        anyhow::Error::new(AlreadyInitialized).context(detail.to_string())
    } else {
        detail.context("initialization database operation failed")
    }
}
const FILES: &[&str] = &[
    "pki/ca.key",
    "pki/server.key",
    "pki/server.crt",
    "secrets/idempotency.key",
    "backup-recipient.txt",
    "pki/ca.crt",
];
fn publish(dir: &DataDir) -> anyhow::Result<()> {
    let stage = dir.root.join(".init-staging");
    for relative in FILES {
        let from = stage.join(relative);
        let to = dir.root.join(relative);
        if from.exists() {
            private_dir(to.parent().unwrap())?;
            fs::rename(&from, &to)?;
            sync_dir(from.parent().unwrap())?;
            sync_dir(to.parent().unwrap())?;
        } else {
            ensure!(to.is_file(), "incomplete committed initialization staging");
        }
    }
    sync_dir(&dir.root)?;
    // The output remains durable until the caller has successfully written and flushed it.
    Ok(())
}
/// Acknowledge successful delivery of InitOutput. Call only after writing and flushing it.
/// Until then initialize recovers the same output, including the original bootstrap token.
pub fn finish_init(dir: &DataDir) -> io::Result<()> {
    let _lock = File::open(&dir.root)?;
    _lock.lock()?;
    let stage = dir.root.join(".init-staging");
    if !stage.exists() {
        return Ok(());
    }
    for relative in FILES {
        if !dir.root.join(relative).is_file() || stage.join(relative).exists() {
            return Err(io::Error::other(
                "initialization has not finished publication",
            ));
        }
    }
    match fs::remove_file(stage.join("init-output.json")) {
        Ok(()) => sync_dir(&stage)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fs::remove_dir_all(&stage)?;
    sync_dir(&dir.root)
}
fn recover(dir: &DataDir, state: Option<&[u8]>) -> anyhow::Result<Option<InitOutput>> {
    let stage = dir.root.join(".init-staging");
    if !stage.exists() {
        return Ok(None);
    }
    let Some(fp) = state else {
        fs::remove_dir_all(&stage)?;
        sync_dir(&dir.root)?;
        return Ok(None);
    };
    let staged_ca = stage.join("pki/ca.crt");
    let cert = read_secret_text(&if staged_ca.exists() {
        staged_ca
    } else {
        dir.ca_cert()
    })?;
    let fingerprint = parse_cert_pem(&cert)?.fingerprint;
    ensure!(
        fingerprint.as_slice() == fp,
        "staging belongs to a different installation"
    );
    if !stage.join("init-output.json").exists() && dir.is_initialized() {
        // A crash during finish_init is already acknowledged, not a new initialization.
        finish_publication_cleanup(dir)?;
        return Err(AlreadyInitialized.into());
    }
    let bytes = Zeroizing::new(fs::read(stage.join("init-output.json"))?);
    let output: InitOutput = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid staged initialization output"))?;
    ensure!(
        output.ca_fingerprint == format_fingerprint(&fingerprint),
        "staged output fingerprint mismatch"
    );
    publish(dir)?;
    Ok(Some(output))
}
// The caller already holds the directory lock; do not try to acquire it recursively.
fn finish_publication_cleanup(dir: &DataDir) -> io::Result<()> {
    fs::remove_dir_all(dir.root.join(".init-staging"))?;
    sync_dir(&dir.root)
}
async fn preflight(
    tx: &mut Transaction<'_, Postgres>,
    dir: &DataDir,
    params: &mut InitParams,
    state: Option<&[u8]>,
) -> anyhow::Result<()> {
    if dir.is_initialized() || state.is_some() {
        return Err(AlreadyInitialized.into());
    }
    let exists =
        sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM porchlight.households) AS "exists!""#)
            .fetch_one(&mut **tx)
            .await
            .map_err(db_error)?;
    if exists {
        return Err(AlreadyInitialized.into());
    }
    validate(params)?;
    params.tzid = porchlight_core::time::storage_zone(&params.tzid)
        .map_err(|_| anyhow::anyhow!("invalid timezone"))?
        .1
        .to_owned();
    let tz_valid = sqlx::query_scalar!(
        "SELECT porchlight.pl_valid_tzid($1) AS \"valid!\"",
        params.tzid
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(db_error)?;
    ensure!(tz_valid, "invalid timezone");
    Ok(())
}
struct Staged {
    output: InitOutput,
    ca: HouseholdCa,
    token: Token,
}
fn stage(dir: &DataDir, params: &InitParams, now: SystemTime) -> anyhow::Result<Staged> {
    let ca = HouseholdCa::generate(&params.household_name, now)?;
    let server = ca.issue_server(&params.sans, now, 730)?;
    let token = Token::generate(TokenKind::Pat);
    let output = InitOutput {
        household_id: Uuid::now_v7(),
        admin_member_id: Uuid::now_v7(),
        ca_fingerprint: format_fingerprint(&ca.cert().fingerprint),
        bootstrap_token: token.expose().to_owned(),
        data_dir: dir.root.clone(),
    };
    let stage = dir.root.join(".init-staging");
    let staged = DataDir::new(&stage);
    let mut random = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *random).map_err(|_| anyhow::anyhow!("OS random source failed"))?;
    let idem = Zeroizing::new(format!("{}\n", hex::encode(random.as_slice())));
    let result = (|| -> anyhow::Result<()> {
        private_dir(&stage)?;
        write_secret_file(&staged.ca_cert(), ca.cert().cert_pem.as_bytes(), true)?;
        write_secret_file(&staged.ca_key(), ca.key_pem().as_bytes(), true)?;
        write_secret_file(&staged.server_cert(), server.cert.cert_pem.as_bytes(), true)?;
        write_secret_file(&staged.server_key(), server.key_pem.as_bytes(), true)?;
        write_secret_file(&staged.idempotency_key(), idem.as_bytes(), true)?;
        write_secret_file(
            &staged.backup_recipient(),
            format!("{}\n", params.age_recipient).as_bytes(),
            true,
        )?;
        let json = Zeroizing::new(serde_json::to_vec(&output)?);
        write_secret_file(&stage.join("init-output.json"), &json, true)?;
        sync_dir(&stage)?;
        sync_dir(&dir.root)?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&stage);
        return Err(e);
    }
    Ok(Staged { output, ca, token })
}
async fn hash_password(
    params: &InitParams,
    hasher: &PasswordHasher,
) -> anyhow::Result<Zeroizing<String>> {
    let (m, t, p) = hasher.params();
    let password = params.admin_password.clone();
    static HASH_SLOTS: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)));
    let permit = HASH_SLOTS.clone().acquire_owned().await?;
    Ok(Zeroizing::new(
        tokio::task::spawn_blocking(move || {
            // Keep the slot until hashing finishes, even if the async caller is cancelled.
            let _permit = permit;
            PasswordHasher::with_params(m, t, p).hash(&password)
        })
        .await
        .map_err(|_| anyhow::anyhow!("password hashing failed"))?,
    ))
}
async fn insert_rows(
    tx: &mut Transaction<'_, Postgres>,
    params: &InitParams,
    staged: &Staged,
    hash: &str,
    now: SystemTime,
) -> anyhow::Result<()> {
    let output = &staged.output;
    let user_id = Uuid::now_v7();
    let expires: DateTime<Utc> = now
        .checked_add(Duration::from_secs(365 * 86400))
        .ok_or_else(|| anyhow::anyhow!("invalid initialization time"))?
        .into();
    let initialized: DateTime<Utc> = now.into();
    let token_hash = staged.token.hash();
    sqlx::query!(
        r#"INSERT INTO porchlight.households (id, name, tzid)
           VALUES ($1, $2, $3)"#,
        output.household_id,
        params.household_name,
        params.tzid
    )
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    sqlx::query!(
        r#"INSERT INTO porchlight.users (id, household_id, username)
           VALUES ($1, $2, $3)"#,
        user_id,
        output.household_id,
        params.admin_username
    )
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    sqlx::query!(
        r#"INSERT INTO porchlight.user_credentials (user_id, household_id, password_hash)
           VALUES ($1, $2, $3)"#,
        user_id,
        output.household_id,
        hash
    )
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    sqlx::query!(
        r#"INSERT INTO porchlight.household_members
               (id, household_id, user_id, role, display_name, color, tzid)
           VALUES ($1, $2, $3, 'super_admin', $4, '#FABD2F', $5)"#,
        output.admin_member_id,
        output.household_id,
        user_id,
        params.admin_display_name,
        params.tzid
    )
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    sqlx::query!(
        r#"INSERT INTO porchlight.api_tokens
               (household_id, owner_member_id, name, token_hash, scopes, expires_at)
           VALUES ($1, $2, 'bootstrap', $3, ARRAY['read','write','admin'], $4)"#,
        output.household_id,
        output.admin_member_id,
        token_hash.as_slice(),
        expires
    )
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    sqlx::query!(
        r#"INSERT INTO porchlight.install_state (id, household_id, ca_fingerprint, initialized_at)
           VALUES (true, $1, $2, $3)"#,
        output.household_id,
        staged.ca.cert().fingerprint.as_slice(),
        initialized
    )
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
}
/// Publish the installation and return output, retaining it durably until finish_init.
pub async fn initialize(
    pool: &PgPool,
    dir: &DataDir,
    params: InitParams,
    hasher: &PasswordHasher,
    now: SystemTime,
) -> anyhow::Result<InitOutput> {
    initialize_inner(pool, dir, params, hasher, now, false).await
}
#[cfg(any(test, feature = "testing"))]
pub async fn initialize_crashing_after_commit(
    pool: &PgPool,
    dir: &DataDir,
    params: InitParams,
    hasher: &PasswordHasher,
    now: SystemTime,
) -> anyhow::Result<InitOutput> {
    initialize_inner(pool, dir, params, hasher, now, true).await
}
async fn initialize_inner(
    pool: &PgPool,
    dir: &DataDir,
    mut params: InitParams,
    hasher: &PasswordHasher,
    now: SystemTime,
    crash: bool,
) -> anyhow::Result<InitOutput> {
    // Do not create a root for invalid/refused init. Repeat checks after acquiring its lock.
    if !dir.root.exists() {
        let already = sqlx::query_scalar!(
            r#"SELECT (EXISTS (SELECT 1 FROM porchlight.install_state)
                    OR EXISTS (SELECT 1 FROM porchlight.households)) AS "already!""#
        )
        .fetch_one(pool)
        .await
        .map_err(db_error)?;
        if already {
            return Err(AlreadyInitialized.into());
        }
        validate(&mut params)?;
        private_dir(&dir.root)?;
    }
    let _directory_lock = lock_directory(&dir.root).await?;
    let actor = porchlight_db::tx::Actor {
        member_id: None,
        device_id: None,
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let mut tx = porchlight_db::tx::begin_as(pool, &actor)
        .await
        .map_err(db_error)?;
    let state =
        sqlx::query_scalar!("SELECT ca_fingerprint FROM porchlight.install_state WHERE id = true")
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
    if let Some(output) = recover(dir, state.as_deref())? {
        tx.commit().await.map_err(db_error)?;
        return Ok(output);
    }
    preflight(&mut tx, dir, &mut params, state.as_deref()).await?;
    private_dir(&dir.root)?;
    let hash = hash_password(&params, hasher).await?;
    let staged = stage(dir, &params, now)?;
    if let Err(e) = insert_rows(&mut tx, &params, &staged, &hash, now).await {
        tx.rollback().await.map_err(db_error)?;
        fs::remove_dir_all(dir.root.join(".init-staging"))?;
        sync_dir(&dir.root)?;
        return Err(e);
    }
    // Failed/ambiguous commit retains staging; the next run consults install_state.
    tx.commit().await.map_err(db_error)?;
    if crash {
        bail!("simulated crash after initialization commit");
    }
    publish(dir)?;
    Ok(staged.output)
}
async fn lock_directory(path: &std::path::Path) -> anyhow::Result<File> {
    let f = File::open(path)?;
    tokio::task::spawn_blocking(move || {
        f.lock()?;
        Ok::<_, std::io::Error>(f)
    })
    .await
    .map_err(|_| anyhow::anyhow!("directory lock failed"))?
    .map_err(Into::into)
}
