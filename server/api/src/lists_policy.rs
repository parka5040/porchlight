//! S7a additions to the shared endpoint matrix, not a competing generator.
use crate::permissions::{Endpoint, Policy, PolicyContext};
use porchlight_core::access::{self, Capability, ListCapability};
fn standalone(p: &PolicyContext, cap: ListCapability) -> u16 {
    let c = access::standalone_list_context(
        p.item.caller,
        p.item.visibility,
        p.item.is_owner,
        p.item.is_viewer,
    );
    if !access::standalone_list_can(&c, ListCapability::View) {
        404
    } else if !access::standalone_list_can(&c, cap) {
        403
    } else {
        200
    }
}
fn view(p: &PolicyContext) -> u16 {
    standalone(p, ListCapability::View)
}
fn manage(p: &PolicyContext) -> u16 {
    standalone(p, ListCapability::Manage)
}
fn edit(p: &PolicyContext) -> u16 {
    standalone(p, ListCapability::Edit)
}
fn check(p: &PolicyContext) -> u16 {
    standalone(p, ListCapability::Check)
}
fn delete(p: &PolicyContext) -> u16 {
    standalone(p, ListCapability::Delete)
}
fn visibility(p: &PolicyContext) -> u16 {
    standalone(p, ListCapability::ChangeVisibility)
}
fn add(p: &PolicyContext) -> u16 {
    match manage(p) {
        200 => 201,
        x => x,
    }
}
fn attached_add(p: &PolicyContext) -> u16 {
    match Policy::Item(Capability::Edit).expected(p) {
        200 => 201,
        404 => 422,
        x => x,
    }
}
fn attached_entry_add(p: &PolicyContext) -> u16 {
    match Policy::Item(Capability::Edit).expected(p) {
        200 => 201,
        x => x,
    }
}
fn attached_viewers(p: &PolicyContext) -> u16 {
    match Policy::Item(Capability::View).expected(p) {
        200 => 403,
        x => x,
    }
}
macro_rules! row {
    ($method:literal,$path:literal,$handler:literal,$policy:expr,$case:literal) => {
        Endpoint {
            method: $method,
            path: $path,
            handler: $handler,
            policy: $policy,
            case: $case,
        }
    };
}
macro_rules! standalone {
    ($label:literal,$f:ident) => {
        Policy::List {
            label: $label,
            expected: $f,
        }
    };
}
macro_rules! attached {
    ($cap:ident, $label:literal) => {
        Policy::List {
            label: $label,
            expected: |p| Policy::Item(Capability::$cap).expected(p),
        }
    };
}
pub const ENDPOINTS: &[Endpoint] = &[
    row!("GET", "/v1/lists", "list", Policy::ListCollection(200), ""),
    row!(
        "POST",
        "/v1/lists",
        "create",
        Policy::ListCollection(201),
        ""
    ),
    row!(
        "POST",
        "/v1/lists",
        "create",
        standalone!("attached edit", attached_add),
        "attached"
    ),
    row!(
        "GET",
        "/v1/lists/{id}",
        "get",
        standalone!("list view", view),
        ""
    ),
    row!(
        "PATCH",
        "/v1/lists/{id}",
        "patch",
        standalone!("list edit", edit),
        ""
    ),
    row!(
        "DELETE",
        "/v1/lists/{id}",
        "delete",
        standalone!("list delete", delete),
        ""
    ),
    row!(
        "POST",
        "/v1/lists/{id}/entries",
        "add_entry",
        standalone!("list manage", add),
        ""
    ),
    row!(
        "PATCH",
        "/v1/list-entries/{id}",
        "patch_entry",
        standalone!("list manage", manage),
        ""
    ),
    row!(
        "DELETE",
        "/v1/list-entries/{id}",
        "delete_entry",
        standalone!("list manage", manage),
        ""
    ),
    row!(
        "POST",
        "/v1/list-entries/{id}/check",
        "check",
        standalone!("list check", check),
        ""
    ),
    row!(
        "POST",
        "/v1/list-entries/{id}/uncheck",
        "uncheck",
        standalone!("list check", check),
        ""
    ),
    row!(
        "POST",
        "/v1/lists/{id}/clear-checked",
        "clear",
        standalone!("list manage", manage),
        ""
    ),
    row!(
        "PUT",
        "/v1/lists/{id}/viewers",
        "viewers",
        standalone!("list change_visibility", visibility),
        ""
    ),
    row!(
        "GET",
        "/v1/lists/{id}",
        "get",
        attached!(View, "view"),
        "attached"
    ),
    row!(
        "PATCH",
        "/v1/lists/{id}",
        "patch",
        attached!(Edit, "edit"),
        "attached"
    ),
    row!(
        "DELETE",
        "/v1/lists/{id}",
        "delete",
        attached!(Delete, "delete"),
        "attached"
    ),
    row!(
        "POST",
        "/v1/lists/{id}/entries",
        "add_entry",
        standalone!("attached edit", attached_entry_add),
        "attached"
    ),
    row!(
        "PATCH",
        "/v1/list-entries/{id}",
        "patch_entry",
        attached!(Edit, "edit"),
        "attached"
    ),
    row!(
        "DELETE",
        "/v1/list-entries/{id}",
        "delete_entry",
        attached!(Edit, "edit"),
        "attached"
    ),
    row!(
        "POST",
        "/v1/list-entries/{id}/check",
        "check",
        attached!(ChangeStatus, "change_status"),
        "attached"
    ),
    row!(
        "POST",
        "/v1/list-entries/{id}/uncheck",
        "uncheck",
        attached!(ChangeStatus, "change_status"),
        "attached"
    ),
    row!(
        "POST",
        "/v1/lists/{id}/clear-checked",
        "clear",
        attached!(Edit, "edit"),
        "attached"
    ),
    row!(
        "PUT",
        "/v1/lists/{id}/viewers",
        "viewers",
        standalone!("inherits item viewers", attached_viewers),
        "attached"
    ),
];
