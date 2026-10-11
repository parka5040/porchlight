//! Central contract: area documents are merged here. Regenerate with `porchlight openapi`.
use utoipa::OpenApi;

#[derive(utoipa::ToSchema)]
pub struct ProblemBody {
    #[schema(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub detail: Option<String>,
    pub current: Option<serde_json::Value>,
    pub min_client_version: Option<String>,
    pub errors: Option<Vec<crate::error::FieldError>>,
}
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Header)]
#[allow(dead_code)]
pub struct CommonHeaders {
    /// Client semver; required even on fallback responses.
    #[param(rename = "Porchlight-Client-Version")]
    client_version: String,
    /// Optional IANA time zone.
    #[param(rename = "Porchlight-TZ")]
    tz: Option<String>,
}
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Header)]
#[allow(dead_code)]
pub struct MutationHeaders {
    /// 1–255 visible ASCII characters. Same key/body replays stored status, headers and bytes.
    #[param(rename = "Idempotency-Key")]
    key: String,
}
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Header)]
#[allow(dead_code)]
pub struct StepUpHeader {
    #[param(rename = "Porchlight-Step-Up")]
    token: String,
}
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Header)]
#[allow(dead_code)]
pub struct IfMatchHeader {
    /// Required version for a versioned update/delete. Not used by M2 trust routes.
    #[param(rename = "If-Match")]
    version: String,
}
#[derive(utoipa::IntoResponses)]
#[allow(dead_code)]
pub enum UniversalProblems {
    #[response(
        status = 400,
        description = "Malformed request, client version or time zone",
        content_type = "application/problem+json"
    )]
    BadRequest(ProblemBody),
    #[response(
        status = 401,
        description = "Authentication or credential no longer valid",
        content_type = "application/problem+json"
    )]
    Unauthorized(ProblemBody),
    #[response(status=409,description="Transient database conflict (including authentication/touch on GET); retry the same request",content_type="application/problem+json",headers(("Retry-After"=u64,description="Seconds before retrying")))]
    TransientConflict(ProblemBody),
    #[response(
        status = 426,
        description = "Client version below the configured minimum",
        content_type = "application/problem+json"
    )]
    Upgrade(ProblemBody),
    #[response(
        status = 500,
        description = "Unexpected error; no internal details disclosed",
        content_type = "application/problem+json"
    )]
    Internal(ProblemBody),
}
#[derive(utoipa::IntoResponses)]
#[allow(dead_code)]
pub enum MutationProblems {
    #[response(status=503,description="Server is updating; mutation not executed, retry the identical request shortly",content_type="application/problem+json",headers(("Retry-After"=u64,description="Seconds before retrying")))]
    Maintenance(ProblemBody),
    #[response(
        status = 408,
        description = "Request body timeout",
        content_type = "application/problem+json"
    )]
    Timeout(ProblemBody),
    #[response(status=409,description="Idempotency in progress or route-specific conflict",content_type="application/problem+json",headers(("Retry-After"=u64,description="Seconds before retrying a transient conflict")))]
    Conflict(ProblemBody),
    #[response(
        status = 422,
        description = "Idempotency key reused with another request, or route-specific validation",
        content_type = "application/problem+json"
    )]
    Unprocessable(ProblemBody),
}
#[derive(utoipa::IntoResponses)]
#[response(status=429,description="Rate limit or credential lockout",content_type="application/problem+json",headers(("Retry-After"=u64,description="Seconds before retrying")))]
#[allow(dead_code)]
pub struct RateLimitProblem(ProblemBody);
#[derive(utoipa::IntoResponses)]
#[response(
    status = 403,
    description = "Forbidden",
    content_type = "application/problem+json"
)]
#[allow(dead_code)]
pub struct ForbiddenProblem(ProblemBody);
#[derive(utoipa::IntoResponses)]
#[response(
    status = 403,
    description = "Forbidden or step-up required",
    content_type = "application/problem+json"
)]
#[allow(dead_code)]
pub struct StepUpProblem(ProblemBody);
#[derive(utoipa::IntoResponses)]
#[response(
    status = 404,
    description = "Resource not found",
    content_type = "application/problem+json"
)]
#[allow(dead_code)]
pub struct NotFoundProblem(ProblemBody);
struct RegistryHeaders<'a>(&'a [crate::routes::Route]);
impl utoipa::Modify for RegistryHeaders<'_> {
    fn modify(&self, doc: &mut utoipa::openapi::OpenApi) {
        use utoipa::IntoParams;
        let mut operations = std::collections::BTreeMap::<_, Vec<_>>::new();
        for route in self.0 {
            operations
                .entry((route.path, route.method))
                .or_default()
                .push(route);
        }
        for registrations in operations.values() {
            let route = registrations[0];
            assert!(
                registrations
                    .iter()
                    .all(|r| r.area == route.area && r.handler == route.handler),
                "shared operations must use the same area/handler"
            );
            let item = doc
                .paths
                .paths
                .get_mut(route.path)
                .expect("registered route documented");
            let op = match route.method {
                "GET" => item.get.as_mut(),
                "POST" => item.post.as_mut(),
                "PUT" => item.put.as_mut(),
                "PATCH" => item.patch.as_mut(),
                "DELETE" => item.delete.as_mut(),
                _ => None,
            }
            .expect("registered method documented");
            op.operation_id = Some(format!("{}_{}", route.area, route.handler));
            op.tags = Some(vec![route.area.to_owned()]);
            if route.area == "release" && matches!(route.handler, "client" | "download") {
                op.responses.responses.remove("426");
            }
            use crate::routes::Surface;
            if registrations.iter().any(|r| r.surface == Surface::Mtls)
                && registrations.iter().any(|r| r.surface == Surface::Agent)
            {
                // Separate requirement objects are alternatives (session OR PAT), not AND.
                use utoipa::openapi::security::SecurityRequirement;
                op.security = Some(vec![
                    SecurityRequirement::new("session", Vec::<String>::new()),
                    SecurityRequirement::new("pat", Vec::<String>::new()),
                ]);
            }
            let params = op.parameters.get_or_insert_with(Vec::new);
            params.extend(
                CommonHeaders::into_params(|| None)
                    .into_iter()
                    .map(Into::into),
            );
            if route.method != "GET" {
                params.extend(
                    MutationHeaders::into_params(|| None)
                        .into_iter()
                        .map(Into::into),
                );
            }
        }
    }
}

#[derive(OpenApi)]
#[openapi(info(title="Porchlight",description="Household API. Application routes require TLS 1.3 mutual TLS with an enrolled household device certificate and a device-bound session bearer. Login/reset require the device certificate but no session. Enrollment uses LAN-only server-auth TLS plus a one-time token and password. Agent routes use PAT bearer over a local Unix socket; an admin-scoped PAT is agent step-up. Terminal logout/self-revoke/PAT-self-revoke retries authenticate first and may return 401 instead of replayed success."),servers((url="https://porchlight.lan:8443",description="LAN mTLS"),(url="https://porchlight.lan:8444",description="LAN enrollment"),(url="http://localhost",description="Agent API via Unix socket /run/porchlight/agent.sock")))]
pub struct Api;
pub fn document() -> utoipa::openapi::OpenApi {
    let mut doc = Api::openapi();
    doc.info.version = porchlight_core::VERSION.to_owned();
    // Cargo has no declared license; omit it rather than emitting an invalid empty license.
    doc.info.license = None;
    doc.merge(crate::routes::auth::AuthApi::openapi());
    doc.merge(crate::routes::devices::DevicesApi::openapi());
    doc.merge(crate::routes::enrollment::EnrollmentApi::openapi());
    doc.merge(crate::routes::admin::AdminApi::openapi());
    doc.merge(crate::routes::security_events::SecurityEventsApi::openapi());
    doc.merge(crate::routes::administration::AdministrationApi::openapi());
    doc.merge(crate::routes::connection::ConnectionApi::openapi());
    doc.merge(crate::routes::me::MeApi::openapi());
    doc.merge(crate::routes::reads::ReadsApi::openapi());
    doc.merge(crate::routes::exports::ExportsApi::openapi());
    doc.merge(crate::routes::backups::BackupsApi::openapi());
    doc.merge(crate::routes::release::ReleaseApi::openapi());
    doc.merge(crate::routes::members::MembersApi::openapi());
    doc.merge(crate::routes::items::ItemsApi::openapi());
    doc.merge(crate::routes::collaboration::CollaborationApi::openapi());
    doc.merge(crate::routes::calendar::CalendarApi::openapi());
    doc.merge(crate::routes::planning::PlanningApi::openapi());
    doc.merge(crate::routes::lists::ListsApi::openapi());
    doc.merge(crate::routes::stream::StreamApi::openapi());
    doc.merge(crate::routes::notifications::NotificationsApi::openapi());
    use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
    let components = doc.components.get_or_insert_with(Default::default);
    components.add_security_scheme(
        "session",
        SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
    );
    components.add_security_scheme(
        "pat",
        SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
    );
    utoipa::Modify::modify(&RegistryHeaders(crate::routes::ROUTES), &mut doc);
    doc
}
pub fn json() -> String {
    let mut s = serde_json::to_string_pretty(&document()).expect("OpenAPI serializes");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dual_surface_operations_are_processed_once() {
        let mut doc = document();
        // Only test paths are new; production /v1/me stays mTLS-only.
        doc.paths = crate::routes::test_registry::dual::DualApi::openapi().paths;
        utoipa::Modify::modify(
            &RegistryHeaders(crate::routes::test_registry::ROUTES),
            &mut doc,
        );
        let json = serde_json::to_value(&doc).unwrap();
        let mut ids = std::collections::BTreeSet::new();
        for (method, handler) in [("get", "read"), ("post", "create")] {
            let op = &json["paths"]["/test/dual"][method];
            assert_eq!(op["operationId"], format!("dual_{handler}"));
            assert!(ids.insert(op["operationId"].as_str().unwrap()));
            let params = op["parameters"].as_array().unwrap();
            let unique: std::collections::BTreeSet<_> = params
                .iter()
                .map(|p| (p["name"].as_str().unwrap(), p["in"].as_str().unwrap()))
                .collect();
            assert_eq!(params.len(), unique.len(), "duplicate shared headers");
            assert_eq!(params.len(), if method == "get" { 2 } else { 3 });
            assert_eq!(
                op["security"],
                serde_json::json!([{"session":[]},{"pat":[]}])
            );
        }
        // Check the synthetic contract's serialization without writing into the source tree.
        let serialized = serde_json::to_string_pretty(&doc).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&serialized).unwrap(),
            json
        );
    }
}
