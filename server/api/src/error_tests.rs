use super::*;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

// Keep a WARN-enabled dispatcher alive process-wide so callsite interest never
// comes from another test thread's no-op default while a scoped capture is live.
// Every scoped capture in this (lib) test binary calls it first.
pub(crate) fn ensure_global_warn_sink() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .with_writer(std::io::sink)
                .finish(),
        )
        .expect("install the test binary's global WARN sink once");
    });
}

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn internal_errors_are_classified_and_redacted(pool: sqlx::PgPool) {
    ensure_global_warn_sink();
    let error = sqlx::query("DO $$ BEGIN RAISE EXCEPTION USING ERRCODE='23514', CONSTRAINT='forced_internal_check', MESSAGE='secret-payload-marker'; END $$")
        .execute(&pool).await.unwrap_err();
    let output = Capture(Arc::new(Mutex::new(Vec::new())));
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let response = tracing::subscriber::with_default(subscriber, || {
        let response = ApiError::from(error).into_response();
        let keys = crate::idempotency::IdempotencyKeys::new(&[1; 32]);
        let sealed = keys.seal("scope", &[], b"secret-token-marker");
        assert!(keys.open("wrong-aad", &sealed).is_err());
        let _ = ApiError::from(sqlx::Error::Io(std::io::Error::other("secret-url-marker")));
        response
    });
    assert_eq!(response.status(), 500);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["detail"].is_null());
    assert_eq!(body["type"], "urn:porchlight:problem:internal");
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    for expected in [
        "ERROR",
        "kind=\"database\"",
        "23514",
        "forced_internal_check",
        "idempotency_decryption",
        "database_io",
    ] {
        assert!(
            text.contains(expected),
            "missing classification: {expected}"
        );
    }
    assert!(!text.contains("secret-"));
    assert!(!String::from_utf8_lossy(&bytes).contains("secret-"));
}

#[test]
#[should_panic(expected = "only conflict may be transient")]
fn transient_rejects_non_conflict_problems() {
    let _ = ApiError::transient(Problem::Forbidden);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn transient_sqlstates_are_warn_logged_and_redacted(pool: sqlx::PgPool) {
    ensure_global_warn_sink();
    assert!(
        tracing::enabled!(tracing::Level::WARN),
        "global WARN interest must precede scoped capture"
    );
    for (code, statement) in [
        (
            "55P03",
            "DO $$ BEGIN RAISE EXCEPTION USING ERRCODE='55P03', CONSTRAINT='secret-constraint-marker', MESSAGE='secret-payload-marker'; END $$",
        ),
        (
            "40001",
            "DO $$ BEGIN RAISE EXCEPTION USING ERRCODE='40001', CONSTRAINT='secret-constraint-marker', MESSAGE='secret-payload-marker'; END $$",
        ),
        (
            "40P01",
            "DO $$ BEGIN RAISE EXCEPTION USING ERRCODE='40P01', CONSTRAINT='secret-constraint-marker', MESSAGE='secret-payload-marker'; END $$",
        ),
    ] {
        let error = sqlx::query(statement).execute(&pool).await.unwrap_err();
        let output = Capture(Arc::new(Mutex::new(Vec::new())));
        let writer = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let error = tracing::subscriber::with_default(subscriber, || ApiError::from(error));
        assert!(error.is_transient());
        assert_eq!(error.problem(), Problem::Conflict);
        assert_eq!(error.retry_after(), Some(1));
        let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert!(
            text.contains("WARN"),
            "transient SQLSTATE must be visible at warn"
        );
        assert!(text.contains(&format!("sqlstate=\"{code}\"")));
        assert!(!text.contains("secret-"));
    }
}

#[test]
fn problem_catalog_is_current() {
    assert_eq!(
        Problem::markdown(),
        include_str!("../../../docs/specs/problems.md"),
        "regenerate docs/specs/problems.md from Problem::markdown()"
    );
}

#[test]
fn field_error_constructors_preserve_optional_code() {
    let plain = serde_json::to_value(FieldError::new("title", "Required")).unwrap();
    assert_eq!(
        plain,
        serde_json::json!({"field":"title", "message":"Required"})
    );
    let coded = serde_json::to_value(
        FieldError::new("after_id", "Invalid reference").with_code("invalid-reference"),
    )
    .unwrap();
    assert_eq!(coded["code"], "invalid-reference");
}

#[test]
fn problem_catalog_explains_new_key_for_changed_sealed_retries() {
    let doc = Problem::markdown();
    assert!(doc.contains("NEW Idempotency-Key"));
    assert!(doc.contains("sealed 422"));
    assert!(doc.contains("stale-neighbour"));
}
