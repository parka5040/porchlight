//! Shared database connection and migration helpers.
use anyhow::{Context, bail};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

/// Server-side limits for app connections: a held lock or a runaway query fails instead of hanging.
pub const APP_SETTINGS: &[(&str, &str)] = &[("lock_timeout", "5s"), ("statement_timeout", "30s")];
/// The migrator waits at most 30 s for a lock (including sqlx's migration lock) instead of hanging.
pub const MIGRATOR_SETTINGS: &[(&str, &str)] = &[("lock_timeout", "30s")];

pub fn connect_options(url: &str, settings: &[(&str, &str)]) -> anyhow::Result<PgConnectOptions> {
    let options: PgConnectOptions = url
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid database URL"))?;
    Ok(options.options(settings.iter().copied()))
}

/// Keep SQLSTATE and constraint metadata, never PostgreSQL's value-bearing message/detail.
pub fn database_error(error: sqlx::Error) -> anyhow::Error {
    if let Some(db) = error.as_database_error() {
        anyhow::anyhow!(
            "database error (SQLSTATE {}, constraint {})",
            db.code().as_deref().unwrap_or("unknown"),
            db.constraint().unwrap_or("none")
        )
    } else {
        error.into()
    }
}
/// Connections use parsed options, so SQLx's I/O cause chain does not contain the URL.
/// Add only the password-free URL, and sanitize any server-supplied database detail.
pub fn connection_error(url: &str, error: sqlx::Error) -> anyhow::Error {
    database_error(error).context(format!(
        "connecting to {}",
        crate::config::redacted_url(url)
    ))
}

/// A private one-slot pool per long-lived LISTEN connection. Reuses the request connection
/// options (role, database and timeouts), never its slots. The listener owns reconnection.
pub(crate) fn listener_pool(requests: &PgPool, name: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_lazy_with((*requests.connect_options()).clone().application_name(name))
}

pub async fn migrate(url: &str) -> anyhow::Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options(url, MIGRATOR_SETTINGS)?)
        .await
        .map_err(|e| connection_error(url, e))
        .context("connecting as migrator")?;
    porchlight_db::MIGRATOR
        .run(&pool)
        .await
        .context("running migrations")?;
    let version = latest_applied(&pool).await?;
    eprintln!("migrated to version {}", version.unwrap_or(0));
    Ok(())
}

pub async fn latest_applied(pool: &PgPool) -> anyhow::Result<Option<i64>> {
    Ok(
        sqlx::query_scalar("select max(version) from public._sqlx_migrations where success")
            .fetch_one(pool)
            .await?,
    )
}

/// Check DB/schema AND an HTTP response from the actual serving process.
/// The local Unix transport is never exposed remotely and needs no stored PAT.
pub async fn healthcheck(url: &str) -> anyhow::Result<()> {
    let socket = std::env::var_os("PORCHLIGHT_AGENT_SOCKET")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/run/porchlight/agent.sock".into());
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        database_health(url).await?;
        serving_health(&socket).await
    })
    .await
    .context("readiness deadline exceeded")?
}

async fn serving_health(socket: &std::path::Path) -> anyhow::Result<()> {
    use http_body_util::{BodyExt, Empty, Limited};
    use hyper::{Request, body::Bytes};
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .context("connecting to serving process")?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await?;
    let task = tokio::spawn(connection);
    // Always abort the driver on response error, cancellation or timeout too.
    struct Driver(tokio::task::JoinHandle<Result<(), hyper::Error>>);
    impl Drop for Driver {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _driver = Driver(task);
    // This deliberately nonexistent route exercises the real router + protocol
    // middleware without adding an unauthenticated domain/registry endpoint.
    let response = sender
        .send_request(
            Request::builder()
                .uri("/internal/readiness")
                .header("host", "localhost")
                .header("porchlight-client-version", env!("CARGO_PKG_VERSION"))
                .body(Empty::<Bytes>::new())?,
        )
        .await?;
    if response.status() != hyper::StatusCode::NOT_FOUND {
        bail!("unexpected serving-process readiness response");
    }
    let bytes = Limited::new(response.into_body(), 4096)
        .collect()
        .await
        .map_err(|_| anyhow::anyhow!("invalid readiness response body"))?
        .to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&bytes)?;
    if problem["status"] != 404 || problem["type"] != "urn:porchlight:problem:not-found" {
        bail!("invalid serving-process readiness response");
    }
    Ok(())
}

async fn database_health(url: &str) -> anyhow::Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(connect_options(url, APP_SETTINGS)?)
        .await
        .map_err(|e| connection_error(url, e))?;
    sqlx::query("select 1")
        .execute(&pool)
        .await
        .context("select 1")?;
    let expected = porchlight_db::MIGRATOR
        .iter()
        .map(|m| m.version)
        .max()
        .context("no migrations embedded")?;
    let applied = latest_applied(&pool)
        .await
        .context("reading public._sqlx_migrations")?;
    if applied != Some(expected) {
        bail!("migrations at version {applied:?}, expected {expected}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{ConnectOptions, Connection};

    #[test]
    fn connection_error_retains_cause_but_not_password() {
        let error = connection_error(
            "postgres://user:s3cret-marker@localhost/db?password=query-secret-marker",
            sqlx::Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "connection refused marker",
            )),
        );
        let text = format!("{error:#}");
        assert!(text.contains("postgres://user@localhost/db"));
        assert!(text.contains("connection refused marker"));
        assert!(!text.contains("s3cret-marker"));
        assert!(!text.contains("query-secret-marker"));
        assert!(error.chain().count() >= 2);
    }

    #[tokio::test]
    async fn connect_options_set_server_side_limits() {
        let url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL is not set; source scripts/env.sh");
        for (settings, want) in [
            (APP_SETTINGS, ("5s", "30s")),
            (MIGRATOR_SETTINGS, ("30s", "0")),
        ] {
            let mut conn = connect_options(&url, settings)
                .unwrap()
                .connect()
                .await
                .unwrap();
            let got: (String, String) = sqlx::query_as(
                "select current_setting('lock_timeout'), current_setting('statement_timeout')",
            )
            .fetch_one(&mut conn)
            .await
            .unwrap();
            assert_eq!((got.0.as_str(), got.1.as_str()), want);
            conn.close().await.unwrap();
        }
    }
}
