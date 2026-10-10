//! S8a policies append to the existing endpoint matrix (no parallel permission framework).
use crate::permissions::{Endpoint, PatScope, Policy, PolicyContext};
use porchlight_core::access::{CallerKind, Role};
fn admin(ctx: &PolicyContext) -> u16 {
    if matches!(ctx.item.caller, CallerKind::AppMember(Role::SuperAdmin))
        || (matches!(ctx.item.caller, CallerKind::Agent) && ctx.pat_scope == Some(PatScope::Admin))
    {
        200
    } else {
        403
    }
}
fn created(ctx: &PolicyContext) -> u16 {
    if admin(ctx) == 200 { 201 } else { 403 }
}
fn queued(ctx: &PolicyContext) -> u16 {
    if admin(ctx) == 200 { 202 } else { 403 }
}
fn self_or_admin(ctx: &PolicyContext) -> u16 {
    if ctx.is_self { 200 } else { admin(ctx) }
}
macro_rules! ep {
    ($method:literal,$path:literal,$handler:literal,$expect:ident) => {
        Endpoint {
            method: $method,
            path: $path,
            handler: $handler,
            policy: Policy::Custom {
                label: "super_admin_or_admin_pat",
                expected: $expect,
            },
            case: "",
        }
    };
}
pub const CALENDAR_ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "POST",
        path: "/v1/imports/ics",
        handler: "import",
        policy: Policy::Custom {
            label: "calendar_self_or_admin",
            expected: self_or_admin,
        },
        case: "",
    },
    ep!("GET", "/v1/feeds", "list", admin),
    ep!("POST", "/v1/feeds", "add", created),
    ep!("PUT", "/v1/feeds/{id}", "update", admin),
    ep!("DELETE", "/v1/feeds/{id}", "remove", admin),
    ep!("POST", "/v1/feeds/{id}/refresh", "refresh", queued),
    ep!("GET", "/v1/admin/feeds", "admin_list", admin),
    Endpoint {
        method: "GET",
        path: "/v1/admin/feeds/target",
        handler: "resolve_target",
        policy: Policy::Custom {
            label: "calendar_self_or_admin",
            expected: self_or_admin,
        },
        case: "",
    },
    ep!("POST", "/v1/admin/feeds", "admin_add", created),
    ep!("PUT", "/v1/admin/feeds/{id}", "admin_update", admin),
    ep!("DELETE", "/v1/admin/feeds/{id}", "admin_remove", admin),
    ep!(
        "POST",
        "/v1/admin/feeds/{id}/refresh",
        "admin_refresh",
        queued
    ),
];
