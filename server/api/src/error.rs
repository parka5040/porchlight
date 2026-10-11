//! RFC 9457 problems. Database/internal failures never reveal their source to callers.
//! Stable extension: `method-not-allowed` (405) covers unsupported methods on known routes.
#[cfg(test)]
#[path = "error_tests.rs"]
pub(crate) mod tests;
use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
/// Closed problem catalog. Add workstream-specific slugs here, never string literals.
/// ```compile_fail
/// use porchlight_api::error::{ApiError, Problem};
/// let _ = ApiError::new(Problem::InvalidRequset);
/// ```
macro_rules! problem_catalog {
    ($($variant:ident => ($slug:literal,$status:literal,$title:literal)),* $(,)?)=>{
        #[derive(Clone,Copy,Debug,PartialEq,Eq)]
        pub enum Problem { $($variant),* }
        impl Problem {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];
            pub const fn slug(self)->&'static str {match self {$(Self::$variant=>$slug),*}}
            pub const fn metadata(self)->(u16,&'static str) {match self {$(Self::$variant=>($status,$title)),*}}
        }
    }
}
problem_catalog! {
    CalendarTooLarge => ("calendar-too-large",422,"Calendar exceeds apply work budget"),
    InvalidRequest => ("invalid-request",400,"Invalid request"),
    IdempotencyKeyRequired => ("idempotency-key-required",400,"Idempotency key required"),
    CsrInvalid => ("csr-invalid",400,"Invalid certificate request"),
    Unauthenticated => ("unauthenticated",401,"Authentication required"),
    DeviceUnknown => ("device-unknown",401,"Unknown device"),
    DeviceCertificateExpired => ("device-certificate-expired",401,"Device certificate expired"),
    RequestTimeout => ("request-timeout",408,"Request body timed out"),
    DeviceRevoked => ("device-revoked",401,"Device revoked"),
    SessionInvalid => ("session-invalid",401,"Invalid session"),
    SessionDeviceMismatch => ("session-device-mismatch",401,"Session belongs to another device"),
    InvalidCredentials => ("invalid-credentials",401,"Invalid credentials"),
    ResetCodeInvalid => ("reset-code-invalid",401,"Invalid reset code"),
    EnrollmentTokenInvalid => ("enrollment-token-invalid",401,"Invalid enrollment token"),
    Forbidden => ("forbidden",403,"Forbidden"),
    StepUpRequired => ("step-up-required",403,"Step-up required"),
    NotFound => ("not-found",404,"Not found"),
    ClientReleaseUnavailable => ("client-release-unavailable",404,"Client release unavailable"),
    MethodNotAllowed => ("method-not-allowed",405,"Method not allowed"),
    IdempotencyInProgress => ("idempotency-in-progress",409,"Request in progress"),
    RenewalTooEarly => ("renewal-too-early",409,"Renewal too early"),
    EnrollmentTokenUsed => ("enrollment-token-used",410,"Enrollment token used"),
    EnrollmentTokenExpired => ("enrollment-token-expired",410,"Enrollment token expired"),
    IdempotencyKeyReused => ("idempotency-key-reused",422,"Idempotency key reused"),
    PasswordPolicy => ("password-policy",422,"Password policy"),
    RateLimited => ("rate-limited",429,"Rate limited"),
    AccountLocked => ("account-locked",429,"Account locked"),
    PurgeDependencyUnhandled => ("purge-dependency-unhandled",409,"Purge requires a retention migration"),
    Internal => ("internal",500,"Internal server error"),
    Maintenance => ("maintenance",503,"Server is updating"),
    // s7b: collaboration/ingest problems
    CommentEditExpired => ("comment-edit-expired",403,"Comment edit window expired"),
    AttachmentTooLarge => ("attachment-too-large",413,"Attachment exceeds 25 MiB limit"),
    StorageFull => ("storage-full",507,"Attachment storage is full"),
    AttachmentTypeUnsupported => ("attachment-type-unsupported",415,"Unsupported attachment type or WebP main meta-Huffman profile"),
    AttachmentTypeMismatch => ("attachment-type-mismatch",422,"Attachment type mismatch"),
    AttachmentInvalid => ("attachment-invalid",422,"Invalid attachment"),
    // s1: mutation protocol problems
    ClientVersionRequired => ("client-version-required",400,"Client version required"),
    UpgradeRequired => ("upgrade-required",426,"Upgrade required"),
    InvalidTimeZone => ("invalid-time-zone",400,"Invalid time zone"),
    IfMatchRequired => ("if-match-required",428,"If-Match required"),
    VersionConflict => ("version-conflict",409,"Version conflict"),
    ValidationFailed => ("validation-failed",422,"Validation failed"),
    Conflict => ("conflict",409,"Conflict"),
    // o2a: resumable encrypted bundle downloads.
    RangeNotSatisfiable => ("range-not-satisfiable",416,"Range not satisfiable"),
}
impl Problem {
    pub fn markdown() -> String {
        let mut doc = String::from(
            "# Problem catalog\n\nGenerated from `server/api/src/error.rs`; RFC 9457 bodies use `urn:porchlight:problem:<slug>`.\n\n| Slug | Status | Title |\n| --- | --- | --- |\n",
        );
        for problem in Self::ALL {
            let (status, title) = problem.metadata();
            doc.push_str(&format!(
                "| `{}` | {} | {} |\n",
                problem.slug(),
                status,
                title
            ));
        }
        doc.push_str("\nExtensions: `upgrade-required` includes `min_client_version`; `version-conflict` includes `current`; `validation-failed` includes `errors: [{field, message, code?}]`. Body references use `validation-failed` (422), with `invalid-reference` or `stale-neighbour` field codes for board bounds; 404 is reserved for hidden/missing path resources. Idempotency keys cover method, path/query and body, **not If-Match**: rotate the key after `version-conflict`, otherwise the stored response replays. All bodies retain `type`, `title`, `status`, `detail`; internal errors never reveal details.\n");
        doc.push_str("\nA sealed 422 (including `stale-neighbour`) replays for its original key. After refetching neighbours or correcting validation, a retry with a changed body needs a **NEW Idempotency-Key**; reusing the old key returns `idempotency-key-reused`.\n");
        doc.push_str("\n`maintenance` (503) rejects mutations before authentication, body reads and idempotency while the installer write fence exists. `Retry-After: 1`; retry the identical request after waiting. Reads and healthchecks remain available.\n");
        doc
    }
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct FieldError {
    pub field: String,
    pub message: String,
    /// Optional stable machine-readable field classification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
            code: None,
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }
}

pub struct ApiError {
    problem: Problem,
    extensions: serde_json::Map<String, serde_json::Value>,
    status: StatusCode,
    slug: &'static str,
    title: &'static str,
    detail: Option<String>,
    retry_after: Option<u64>,
    transient: bool,
    apply_timeout: bool,
}
impl ApiError {
    pub fn problem(&self) -> Problem {
        self.problem
    }
    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub fn slug(&self) -> &'static str {
        self.slug
    }
    pub fn retry_after(&self) -> Option<u64> {
        self.retry_after
    }
    pub fn new(problem: Problem) -> Self {
        let (status, title) = problem.metadata();
        Self {
            problem,
            extensions: if problem == Problem::ValidationFailed {
                let mut extensions = serde_json::Map::new();
                extensions.insert("errors".into(), serde_json::json!([]));
                extensions
            } else {
                serde_json::Map::new()
            },
            status: StatusCode::from_u16(status).expect("valid status"),
            slug: problem.slug(),
            title,
            detail: None,
            retry_after: None,
            transient: false,
            apply_timeout: false,
        }
    }
    /// A retryable domain failure: the mutation wrapper rolls back all effects and
    /// the claim, without sealing this response. Retry-After alone does not do this.
    pub fn transient_conflict() -> Self {
        let mut error = Self::new(Problem::Conflict);
        error.transient = true;
        error
    }
    pub fn calendar_apply_timeout() -> Self {
        let mut error = Self::transient_conflict();
        error.apply_timeout = true;
        error
    }
    pub fn is_calendar_apply_timeout(&self) -> bool {
        self.apply_timeout
    }
    pub fn is_transient(&self) -> bool {
        self.transient
    }
    /// RFC problem fields are reserved; only domain extensions can be supplied.
    pub fn extension(mut self, key: &str, value: serde_json::Value) -> Self {
        if matches!(key, "type" | "title" | "status" | "detail") {
            return self;
        }
        self.extensions.insert(key.to_owned(), value);
        self
    }
    pub fn validation(errors: Vec<FieldError>) -> Self {
        Self::new(Problem::ValidationFailed).extension(
            "errors",
            serde_json::to_value(errors).expect("field errors serialize"),
        )
    }
    /// Only static classifications are logged, never source Display/Debug or request data.
    pub fn internal(kind: &'static str) -> Self {
        tracing::error!(kind, "internal API failure");
        Self::new(Problem::Internal)
    }
    pub fn retry(mut self, seconds: u64) -> Self {
        self.retry_after = Some(seconds.max(1));
        self
    }
    pub fn detail(mut self, detail: &str) -> Self {
        if !self.status.is_server_error() {
            self.detail = Some(detail.to_owned());
        }
        self
    }
}
impl std::fmt::Debug for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiError")
            .field("status", &self.status)
            .field("slug", &self.slug)
            .field("detail", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.slug, self.status.as_u16())
    }
}
impl std::error::Error for ApiError {}
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db) = error.as_database_error() {
            // NOWAIT / lock timeout, serialization failure and deadlock abort the
            // transaction. Domain callbacks using `?` must never seal in that state.
            if matches!(db.code().as_deref(), Some("55P03" | "40001" | "40P01")) {
                tracing::warn!(
                    kind = "database_transient",
                    sqlstate = db.code().as_deref().unwrap_or("unknown"),
                    "retryable API transaction failure"
                );
                return Self::transient_conflict().retry(1);
            }
            tracing::error!(
                kind = "database",
                sqlstate = db.code().as_deref().unwrap_or("unknown"),
                constraint = db.constraint().unwrap_or("none"),
                "internal API failure"
            );
            Self::new(Problem::Internal)
        } else {
            let kind = match error {
                sqlx::Error::Io(_) => "database_io",
                sqlx::Error::Tls(_) => "database_tls",
                sqlx::Error::PoolTimedOut => "database_pool_timeout",
                sqlx::Error::PoolClosed => "database_pool_closed",
                sqlx::Error::RowNotFound => "database_row_not_found",
                sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "database_decode",
                _ => "database_other",
            };
            Self::internal(kind)
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({"type":format!("urn:porchlight:problem:{}",self.slug),"title":self.title,"status":self.status.as_u16(),"detail":if self.status.is_server_error(){None}else{self.detail}});
        body.as_object_mut()
            .expect("problem object")
            .extend(self.extensions);
        let mut r = (self.status, Json(body)).into_response();
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/problem+json"),
        );
        if self.status == StatusCode::REQUEST_TIMEOUT
            || self.status == StatusCode::TOO_MANY_REQUESTS
        {
            r.headers_mut().insert(
                header::CONNECTION,
                header::HeaderValue::from_static("close"),
            );
        }
        if let Some(retry) = self.retry_after {
            r.headers_mut().insert(
                header::RETRY_AFTER,
                retry.to_string().parse().expect("numeric header"),
            );
        }
        r
    }
}
