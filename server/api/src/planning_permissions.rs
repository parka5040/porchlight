//! S8b extends the shared Policy/endpoint generator; no second authorization matrix.
use crate::permissions::{Endpoint, PatScope, Policy, PolicyContext};
use porchlight_core::access::{CallerKind, Capability, Role};
fn private(c: &PolicyContext) -> u16 {
    if c.is_self
        || matches!(
            c.item.caller,
            CallerKind::Agent | CallerKind::AppMember(Role::SuperAdmin)
        )
    {
        200
    } else {
        403
    }
}
fn run(c: &PolicyContext) -> u16 {
    if c.is_self { 200 } else { 403 }
}
fn dependent_run(c: &PolicyContext) -> u16 {
    // check_run_member requires administrative authority for another member.
    if matches!(c.item.caller, CallerKind::Agent) && c.pat_scope != Some(PatScope::Admin) {
        return 403;
    }
    if matches!(
        c.item.caller,
        CallerKind::Agent | CallerKind::AppMember(Role::SuperAdmin)
    ) {
        200
    } else {
        403
    }
}
fn accept(c: &PolicyContext) -> u16 {
    let view = Policy::Item(Capability::View).expected(c);
    if view != 200 {
        view
    } else if Policy::Item(Capability::Edit).expected(c) == 200 || c.is_self {
        200
    } else {
        403
    }
}
fn dismiss(c: &PolicyContext) -> u16 {
    if private(c) != 200 {
        403
    } else {
        Policy::Item(Capability::View).expected(c)
    }
}
pub const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/v1/members/{id}/availability",
        handler: "availability",
        policy: Policy::Custom {
            label: "self/admin; independent of item visibility",
            expected: private,
        },
        case: "",
    },
    Endpoint {
        method: "PUT",
        path: "/v1/members/{id}/availability",
        handler: "put_availability",
        policy: Policy::Custom {
            label: "self/admin; independent of item visibility",
            expected: private,
        },
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/schedule/suggest",
        handler: "suggest",
        policy: Policy::Custom {
            label: "self only; member agenda inputs, caller visibility filters response",
            expected: run,
        },
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/schedule/suggest",
        handler: "suggest",
        policy: Policy::Custom {
            label: "super admin for dependent; member agenda inputs",
            expected: dependent_run,
        },
        case: "dependent",
    },
    Endpoint {
        method: "GET",
        path: "/v1/suggestions",
        handler: "suggestions",
        policy: Policy::Custom {
            label: "self/admin; item ACL filters rows",
            expected: private,
        },
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/suggestions/{id}/accept",
        handler: "accept",
        policy: Policy::CustomItem {
            label: "item edit OR own agenda member; future proposal",
            expected: accept,
        },
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/suggestions/{id}/dismiss",
        handler: "dismiss",
        policy: Policy::CustomItem {
            label: "self/admin + item view",
            expected: dismiss,
        },
        case: "",
    },
    Endpoint {
        method: "GET",
        path: "/v1/items/{id}/candidates",
        handler: "candidates",
        policy: Policy::Item(Capability::View),
        case: "",
    },
];
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn member_policy_is_independent_of_item_visibility() {
        for mut row in crate::permissions::matrix_rows(ENDPOINTS) {
            if row.endpoint.handler == "availability"
                || row.endpoint.handler == "put_availability"
                || row.endpoint.handler == "suggest"
                || row.endpoint.handler == "suggestions"
            {
                row.context.visibility = porchlight_core::access::Visibility::Private;
                let context = PolicyContext {
                    item: row.context,
                    is_self: true,
                    pat_scope: row.pat_scope,
                };
                if row.endpoint.case != "dependent" {
                    assert_eq!(row.endpoint.policy.expected(&context), 200);
                }
                let baseline = row.endpoint.policy.expected(&PolicyContext {
                    is_self: false,
                    ..context
                });
                for visibility in [
                    porchlight_core::access::Visibility::Household,
                    porchlight_core::access::Visibility::Selected,
                ] {
                    let mut other = context;
                    other.item.visibility = visibility;
                    other.is_self = false;
                    assert_eq!(row.endpoint.policy.expected(&other), baseline);
                }
            }
        }
    }
}
