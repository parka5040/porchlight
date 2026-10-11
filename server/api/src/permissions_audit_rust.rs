use super::{Endpoint, endpoint_tables, tests::assert_endpoint_coverage};
use crate::routes::{ROUTES, Route, Surface, Terminal};

// Trust/bootstrap handlers have dedicated trust HTTP tests, not domain policy rows.
// Exempt exact registrations, never whole areas, path prefixes or legacy methods.
const TRUST_EXEMPTIONS: &[(Surface, &str, &str, &str)] = &[
    (Surface::Mtls, "GET", "/v1/connection", "get"),
    (Surface::Mtls, "GET", "/v1/me", "get"),
    (Surface::Mtls, "POST", "/v1/auth/login", "login"),
    (Surface::Mtls, "POST", "/v1/auth/logout", "logout"),
    (Surface::Mtls, "POST", "/v1/auth/password", "password"),
    (Surface::Mtls, "POST", "/v1/auth/reauth", "reauth"),
    (
        Surface::Mtls,
        "POST",
        "/v1/auth/reset-password",
        "reset_password",
    ),
    (
        Surface::Mtls,
        "POST",
        "/v1/devices/self/certificate",
        "certificate",
    ),
    (Surface::Mtls, "GET", "/v1/devices", "list"),
    (Surface::Mtls, "POST", "/v1/devices/{id}/revoke", "revoke"),
    (Surface::Mtls, "GET", "/v1/endpoints", "endpoints"),
    (
        Surface::Enrollment,
        "POST",
        "/v1/enrollment/complete",
        "complete",
    ),
    (Surface::Agent, "GET", "/v1/admin/whoami", "whoami"),
    (
        Surface::Agent,
        "POST",
        "/v1/admin/enrollment-tokens",
        "enrollment_tokens",
    ),
    (
        Surface::Agent,
        "POST",
        "/v1/admin/users/{username}/password-reset",
        "password_reset",
    ),
    (Surface::Agent, "GET", "/v1/admin/devices", "list_devices"),
    (
        Surface::Agent,
        "POST",
        "/v1/admin/devices/{id}/revoke",
        "revoke_device",
    ),
    (Surface::Agent, "POST", "/v1/admin/tokens", "create_token"),
    (
        Surface::Agent,
        "POST",
        "/v1/admin/tokens/{id}/revoke",
        "revoke_token",
    ),
];

pub(super) fn assert_registered_routes_have_policy(routes: &[Route], tables: &[&[Endpoint]]) {
    for r in routes {
        let exempt = TRUST_EXEMPTIONS.contains(&(r.surface, r.method, r.path, r.handler));
        let covered = tables.iter().any(|table| {
            table.iter().any(|e| {
                e.case.is_empty()
                    && e.method == r.method
                    && e.path == r.path
                    && e.handler == r.handler
                    && super::endpoint_surfaces(e).contains(&r.surface)
            })
        });
        assert!(covered || exempt, "route missing base policy: {r:?}");
    }
}

#[test]
fn trust_exemptions_are_registered_and_not_already_covered() {
    for &(surface, method, path, handler) in TRUST_EXEMPTIONS {
        assert!(
            ROUTES.iter().any(|r| {
                r.surface == surface && r.method == method && r.path == path && r.handler == handler
            }),
            "stale trust exemption: {surface:?} {method} {path} {handler}"
        );
        assert!(
            !endpoint_tables().iter().any(|table| table.iter().any(|e| {
                e.method == method
                    && e.path == path
                    && e.handler == handler
                    && super::endpoint_surfaces(e).contains(&surface)
            })),
            "policy row supersedes trust exemption: {method} {path}"
        );
    }
}

fn fixture() -> Route {
    Route {
        surface: Surface::Mtls,
        area: "reads",
        method: "GET",
        path: "/v1/review-fixture",
        handler: "review_fixture",
        legacy: false,
        terminal: Terminal::None,
    }
}

fn assert_rejected(route: Route) {
    let mut routes = ROUTES.to_vec();
    routes.push(route);
    assert!(
        std::panic::catch_unwind(|| assert_endpoint_coverage(&routes, &endpoint_tables())).is_err(),
        "registered route without a base policy was accepted: {route:?}"
    );
}

#[test]
fn new_domain_route_requires_policy() {
    assert_rejected(fixture());
}

#[test]
fn new_trust_route_is_not_implicitly_exempt() {
    for area in ["auth", "enrollment", "devices", "admin", "connection", "me"] {
        for surface in [Surface::Mtls, Surface::Enrollment, Surface::Agent] {
            assert_rejected(Route {
                area,
                surface,
                legacy: true,
                ..fixture()
            });
        }
    }
}

#[test]
fn coverage_includes_method_handler_and_surface() {
    for route in [
        Route {
            method: "PUT",
            path: "/v1/items",
            handler: "list",
            ..fixture()
        },
        Route {
            path: "/v1/items",
            handler: "different_handler",
            ..fixture()
        },
        Route {
            surface: Surface::Enrollment,
            path: "/v1/items",
            handler: "list",
            ..fixture()
        },
    ] {
        assert_rejected(route);
    }
}

#[test]
fn request_variant_cannot_substitute_for_base_policy() {
    let mut endpoints = super::READ_ENDPOINTS.to_vec();
    let endpoint = endpoints
        .iter_mut()
        .find(|e| e.path == "/v1/board")
        .unwrap();
    endpoint.case = "only-a-variant";
    let tables: Vec<&[Endpoint]> = endpoint_tables()
        .into_iter()
        .map(|table| {
            if table.iter().any(|e| e.path == "/v1/board") {
                endpoints.as_slice()
            } else {
                table
            }
        })
        .collect();
    assert!(
        std::panic::catch_unwind(|| assert_endpoint_coverage(ROUTES, &tables)).is_err(),
        "request variant was accepted without a base policy"
    );
}

#[test]
fn exemptions_match_the_complete_registration_tuple() {
    let route = *ROUTES
        .iter()
        .find(|r| r.path == "/v1/enrollment/complete")
        .unwrap();
    for changed in [
        Route {
            surface: Surface::Mtls,
            ..route
        },
        Route {
            method: "GET",
            ..route
        },
        Route {
            handler: "another_handler",
            ..route
        },
    ] {
        assert_rejected(changed);
    }
}
