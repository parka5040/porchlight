//! Extensible endpoint policy table; documentation and real HTTP matrix tests use these rows.
use porchlight_core::access::{self, AccessContext, CallerKind, Capability, Role, Visibility};
#[derive(Clone, Copy, Debug)]
pub struct Endpoint {
    pub method: &'static str,
    pub path: &'static str,
    pub handler: &'static str,
    pub policy: Policy,
    /// Optional request variant for additional permission branches of one route.
    pub case: &'static str,
}
/// Additional endpoint areas supply their own pure expectation function. Context
/// carries resource-independent facts alongside the optional item ACL dimension.
#[derive(Clone, Copy, Debug)]
pub struct PolicyContext {
    pub item: AccessContext,
    pub is_self: bool,
    /// None for app callers; agent scopes are independent of member/item relationships.
    pub pat_scope: Option<PatScope>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PatScope {
    Read,
    Write,
    Admin,
}
impl PatScope {
    pub const ALL: [Self; 3] = [Self::Read, Self::Write, Self::Admin];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Admin => "admin",
        }
    }
    pub fn scopes(self) -> Vec<String> {
        vec![self.as_str().to_owned()]
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Policy {
    Item(Capability),
    /// An item collection operation: preserve the item fixture dimension.
    ItemCollection(u16),
    /// Item ACL crossed with independent resource authorship and exact PAT scopes.
    ItemResource {
        label: &'static str,
        expected: fn(&PolicyContext) -> u16,
    },
    /// List ACLs (standalone or inherited): visibility/relationship × scope × self.
    List {
        label: &'static str,
        expected: fn(&PolicyContext) -> u16,
    },
    ListCollection(u16),
    Authenticated(u16),
    /// SSE ACL dimension without treating delivery as an HTTP item operation.
    StreamResource {
        /// Member-scoped payloads cross recipient equality independently of item ownership.
        member_scoped: bool,
        label: &'static str,
        expected: fn(&PolicyContext) -> u16,
    },
    /// Custom item policies retain ACL dimensions and independently cross target
    /// self/other and all PAT scopes (planning actions are private item resources).
    CustomItem {
        label: &'static str,
        expected: fn(&PolicyContext) -> u16,
    },
    /// Custom callbacks must enforce any admin PAT-scope obligation themselves;
    /// the generic precheck only enforces read/write by HTTP method.
    Custom {
        label: &'static str,
        expected: fn(&PolicyContext) -> u16,
    },
}
impl Policy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Item(cap) => cap.as_str(),
            Self::Authenticated(_) | Self::ItemCollection(_) | Self::ListCollection(_) => {
                "authenticated"
            }
            Self::Custom { label, .. }
            | Self::CustomItem { label, .. }
            | Self::List { label, .. }
            | Self::ItemResource { label, .. }
            | Self::StreamResource { label, .. } => label,
        }
    }
    pub fn expected(self, context: &PolicyContext) -> u16 {
        match self {
            Self::Item(cap) => {
                if !access::can_view(&context.item) {
                    404
                } else if !access::capabilities(&context.item).contains(cap)
                    || (cap == Capability::Take && !context.item.unassigned_dependent_item)
                {
                    403
                } else {
                    200
                }
            }
            Self::Authenticated(success)
            | Self::ItemCollection(success)
            | Self::ListCollection(success) => success,
            Self::Custom { expected, .. }
            | Self::CustomItem { expected, .. }
            | Self::List { expected, .. }
            | Self::ItemResource { expected, .. }
            | Self::StreamResource { expected, .. } => expected(context),
        }
    }
}
pub const ITEM_ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/v1/items",
        handler: "list",
        policy: Policy::ItemCollection(200),
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/items",
        handler: "create",
        policy: Policy::ItemCollection(201),
        case: "",
    },
    Endpoint {
        method: "GET",
        path: "/v1/items/{id}",
        handler: "get",
        policy: Policy::Item(Capability::View),
        case: "",
    },
    Endpoint {
        method: "PATCH",
        path: "/v1/items/{id}",
        handler: "patch",
        policy: Policy::Item(Capability::Edit),
        case: "",
    },
    Endpoint {
        method: "DELETE",
        path: "/v1/items/{id}",
        handler: "delete",
        policy: Policy::Item(Capability::Delete),
        case: "",
    },
    Endpoint {
        method: "PUT",
        path: "/v1/items/{id}/assignees",
        handler: "assignees",
        policy: Policy::Item(Capability::Reassign),
        case: "",
    },
    Endpoint {
        method: "PUT",
        path: "/v1/items/{id}/viewers",
        handler: "viewers",
        policy: Policy::Item(Capability::ChangeVisibility),
        case: "",
    },
    Endpoint {
        method: "PUT",
        path: "/v1/items/{id}/reminders",
        handler: "reminders",
        policy: Policy::Item(Capability::EditReminders),
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/items/{id}/take",
        handler: "take",
        policy: Policy::Item(Capability::Take),
        case: "",
    },
    Endpoint {
        method: "GET",
        path: "/v1/items/{id}/occurrences",
        handler: "occurrences",
        policy: Policy::Item(Capability::View),
        case: "",
    },
    Endpoint {
        method: "PATCH",
        path: "/v1/occurrences/{id}",
        handler: "patch_occurrence",
        policy: Policy::Item(Capability::ChangeStatus),
        case: "",
    },
    Endpoint {
        method: "PATCH",
        path: "/v1/occurrences/{id}",
        handler: "patch_occurrence",
        policy: Policy::Item(Capability::Edit),
        case: "override",
    },
    Endpoint {
        method: "PATCH",
        path: "/v1/occurrences/{id}",
        handler: "patch_occurrence",
        policy: Policy::Item(Capability::ChangeStatus),
        case: "board",
    },
    Endpoint {
        method: "POST",
        path: "/v1/occurrences/{id}/complete",
        handler: "complete",
        policy: Policy::Item(Capability::Complete),
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/occurrences/{id}/reopen",
        handler: "reopen",
        policy: Policy::Item(Capability::ChangeStatus),
        case: "",
    },
];
fn self_member(context: &PolicyContext) -> u16 {
    if context.is_self { 200 } else { 403 }
}
fn self_device_authority(context: &PolicyContext) -> u16 {
    if context.is_self && matches!(context.item.caller, CallerKind::AppMember(_)) {
        200
    } else {
        403
    }
}
pub const READ_ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/v1/board",
        handler: "board",
        case: "",
        policy: Policy::Authenticated(200),
    },
    Endpoint {
        method: "GET",
        path: "/v1/agenda",
        handler: "agenda",
        case: "",
        policy: Policy::Authenticated(200),
    },
    Endpoint {
        method: "GET",
        path: "/v1/snapshot",
        handler: "snapshot",
        case: "",
        policy: Policy::Authenticated(200),
    },
    Endpoint {
        method: "GET",
        path: "/v1/search",
        handler: "search",
        case: "",
        policy: Policy::Authenticated(200),
    },
    Endpoint {
        method: "GET",
        path: "/v1/members",
        handler: "list",
        case: "",
        policy: Policy::Authenticated(200),
    },
    Endpoint {
        method: "PATCH",
        path: "/v1/members/{id}",
        handler: "patch",
        case: "",
        policy: Policy::Custom {
            label: "self-member",
            expected: self_member,
        },
    },
    Endpoint {
        method: "PATCH",
        path: "/v1/members/{id}",
        handler: "patch",
        case: "authority",
        policy: Policy::Custom {
            label: "self-device-authority",
            expected: self_device_authority,
        },
    },
    Endpoint {
        method: "GET",
        path: "/v1/me/settings",
        handler: "settings_get",
        case: "",
        policy: Policy::Authenticated(200),
    },
    Endpoint {
        method: "PUT",
        path: "/v1/me/settings",
        handler: "settings_put",
        case: "",
        policy: Policy::Authenticated(200),
    },
];
#[derive(Clone, Copy, Debug)]
pub enum Relationship {
    Owner,
    Assignee,
    Viewer,
    Subject,
    Other,
    SuperAdmin,
    Agent,
}
impl Relationship {
    pub const ALL: [Self; 7] = [
        Self::Owner,
        Self::Assignee,
        Self::Viewer,
        Self::Subject,
        Self::Other,
        Self::SuperAdmin,
        Self::Agent,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Assignee => "assignee",
            Self::Viewer => "viewer",
            Self::Subject => "subject_only",
            Self::Other => "other_user",
            Self::SuperAdmin => "super_admin_app",
            Self::Agent => "agent",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct EndpointRow {
    pub endpoint: Endpoint,
    pub relationship: Relationship,
    pub visibility: Visibility,
    pub context: AccessContext,
    /// Target member or resource author/uploader; never inferred from an item ACL.
    pub is_self: bool,
    pub pat_scope: Option<PatScope>,
    /// Non-item rows deliberately have no visibility or item-relationship dimension.
    pub item_dimension: bool,
    pub status: u16,
}
fn sse_session(c: &PolicyContext) -> u16 {
    if c.item.caller == CallerKind::Agent {
        404
    } else {
        200
    }
}
fn sse_item(c: &PolicyContext) -> u16 {
    if c.item.caller == CallerKind::Agent {
        404
    } else if access::can_view(&c.item) {
        200
    } else {
        204
    }
}
fn sse_member(c: &PolicyContext) -> u16 {
    if c.item.caller == CallerKind::Agent {
        404
    } else if c.is_self {
        200
    } else {
        204
    }
}
fn sse_list(c: &PolicyContext) -> u16 {
    if c.item.caller == CallerKind::Agent {
        return 404;
    }
    let list = access::standalone_list_context(
        c.item.caller,
        c.item.visibility,
        c.item.is_owner,
        c.item.is_viewer,
    );
    if access::standalone_list_can(&list, access::ListCapability::View) {
        200
    } else {
        204
    }
}
macro_rules! sse_row {
    ($case:literal, $label:literal, $expected:ident) => {
        sse_row!(Custom, $case, $label, $expected)
    };
    (@resource, $case:literal, $label:literal, $expected:ident) => {
        sse_row!(@scope, false, $case, $label, $expected)
    };
    (@member, $case:literal, $label:literal, $expected:ident) => {
        sse_row!(@scope, true, $case, $label, $expected)
    };
    (@scope, $member:literal, $case:literal, $label:literal, $expected:ident) => {
        Endpoint {
            method: "GET", path: "/v1/stream", handler: "get", case: $case,
            policy: Policy::StreamResource {
                member_scoped: $member, label: $label, expected: $expected,
            },
        }
    };
    ($variant:ident, $case:literal, $label:literal, $expected:ident) => {
        Endpoint {
            method: "GET",
            path: "/v1/stream",
            handler: "get",
            case: $case,
            policy: Policy::$variant {
                label: $label,
                expected: $expected,
            },
        }
    };
}
/// 200 = tickle delivered, 204 = filtered (not an HTTP status), 404 = no PAT stream.
/// Standalone-list rows use the same core policy as S7a and the installed SQL ACL.
pub const STREAM_ENDPOINTS: &[Endpoint] = &[
    sse_row!("", "session-only SSE", sse_session),
    sse_row!(@resource, "SSE item.changed", "current item ACL", sse_item),
    sse_row!(@resource, "SSE item.access_changed", "current item ACL", sse_item),
    sse_row!(
        @resource,
        "SSE item.access_changed (former reader)",
        "eviction, no content",
        sse_session
    ),
    sse_row!(@resource, "SSE occurrence.changed", "current parent ACL", sse_item),
    sse_row!(@resource, "SSE derived", "current item ACL", sse_item),
    sse_row!(@resource,
        "SSE list.changed (attached)",
        "current parent ACL",
        sse_item
    ),
    sse_row!(@resource,
        "SSE list_entry.changed (attached)",
        "current parent ACL",
        sse_item
    ),
    sse_row!(@resource,
        "SSE list.changed (standalone)",
        "standalone list ACL",
        sse_list
    ),
    sse_row!(@resource,
        "SSE list_entry.changed (standalone)",
        "standalone list ACL",
        sse_list
    ),
    sse_row!(@resource,
        "SSE list.changed (former reader)",
        "eviction, no content",
        sse_session
    ),
    sse_row!(@resource, "SSE comment.changed", "current parent ACL", sse_item),
    sse_row!(@resource, "SSE attachment.changed", "current parent ACL", sse_item),
    sse_row!(@member,
        "SSE notification.created",
        "recipient equality, IDs only",
        sse_member
    ),
    sse_row!(@member,
        "SSE notification.updated",
        "recipient equality, IDs only",
        sse_member
    ),
    sse_row!(@member,
        "SSE notification.dismissed",
        "recipient equality, IDs only",
        sse_member
    ),
    sse_row!("SSE settings.changed", "member equality", sse_member),
    sse_row!("SSE ics_source.changed", "member equality", sse_member),
    sse_row!("SSE suggestion.changed", "member equality", sse_member),
    sse_row!(
        "SSE member.changed",
        "household broadcast, IDs only",
        sse_session
    ),
];
/// Area registries append tables here, preserving one generator.
// s6a: notification rows are recipient-private, independently of app super-admin role.
fn notification_recipient(c: &PolicyContext) -> u16 {
    if c.is_self { 200 } else { 404 }
}
fn notification_admin(c: &PolicyContext) -> u16 {
    if matches!(c.item.caller, CallerKind::Agent) {
        if c.pat_scope == Some(PatScope::Admin) {
            200
        } else {
            403
        }
    } else {
        404
    }
}
pub const NOTIFICATION_ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/v1/notifications",
        handler: "list",
        policy: Policy::Authenticated(200),
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/notifications/{id}/read",
        handler: "read",
        policy: Policy::Custom {
            label: "recipient-private",
            expected: notification_recipient,
        },
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/notifications/read-all",
        handler: "read_all",
        policy: Policy::Authenticated(200),
        case: "",
    },
    Endpoint {
        method: "GET",
        path: "/v1/me/notification-prefs",
        handler: "prefs_get",
        policy: Policy::Authenticated(200),
        case: "",
    },
    Endpoint {
        method: "PUT",
        path: "/v1/me/notification-prefs",
        handler: "prefs_put",
        policy: Policy::Authenticated(200),
        case: "",
    },
    Endpoint {
        method: "GET",
        path: "/v1/admin/notifications",
        handler: "admin_list",
        policy: Policy::Custom {
            label: "admin PAT inspection (agent-only surface)",
            expected: notification_admin,
        },
        case: "",
    },
];
/// Transport contract: domain endpoints are dual-surface; /v1/admin inspection is agent-only;
/// SSE is delivered only over authenticated member TLS. Does not change Endpoint initializer shape.
pub fn endpoint_surfaces(e: &Endpoint) -> &'static [crate::routes::Surface] {
    use crate::routes::Surface;
    if e.path == "/v1/admin/export/household" {
        &[Surface::Mtls, Surface::Agent]
    } else if e.path.starts_with("/v1/backups")
        || e.path == "/v1/stream"
        || e.path == "/v1/server-release"
        || e.path.starts_with("/v1/client-release")
        || e.path.starts_with("/v1/feeds")
    {
        &[Surface::Mtls]
    } else if e.path.starts_with("/v1/admin/") {
        &[Surface::Agent]
    } else {
        &[Surface::Mtls, Surface::Agent]
    }
}
pub fn endpoint_tables() -> Vec<&'static [Endpoint]> {
    vec![
        ITEM_ENDPOINTS,
        READ_ENDPOINTS,
        crate::routes::exports::ENDPOINTS,
        crate::routes::backups::ENDPOINTS,
        crate::routes::release::ENDPOINTS,
        crate::routes::admin::ENDPOINTS,
        crate::routes::security_events::ENDPOINTS,
        crate::calendar::permissions::CALENDAR_ENDPOINTS,
        crate::lists_policy::ENDPOINTS,
        crate::routes::administration::ENDPOINTS,
        NOTIFICATION_ENDPOINTS,
        crate::routes::collaboration::ENDPOINTS,
        STREAM_ENDPOINTS,
        crate::planning_permissions::ENDPOINTS,
    ]
}
pub fn matrix_rows(endpoints: &[Endpoint]) -> Vec<EndpointRow> {
    let mut rows = Vec::new();
    for &endpoint in endpoints {
        let list_dimension = matches!(
            endpoint.policy,
            Policy::List { .. } | Policy::ListCollection(_) | Policy::ItemResource { .. }
        );
        if !list_dimension
            && !matches!(
                endpoint.policy,
                Policy::Item(_)
                    | Policy::ItemCollection(_)
                    | Policy::StreamResource { .. }
                    | Policy::CustomItem { .. }
            )
        {
            for (relationship, caller, pat_scope) in [
                (Relationship::Other, CallerKind::AppMember(Role::User), None),
                (
                    Relationship::SuperAdmin,
                    CallerKind::AppMember(Role::SuperAdmin),
                    None,
                ),
            ]
            .into_iter()
            .chain(
                PatScope::ALL
                    .into_iter()
                    .map(|scope| (Relationship::Agent, CallerKind::Agent, Some(scope))),
            ) {
                for is_self in [true, false] {
                    if endpoint.path == "/v1/me/settings" && !is_self {
                        continue; // This route has no target-member parameter.
                    }
                    let context = AccessContext {
                        caller,
                        visibility: Visibility::Household,
                        is_owner: false,
                        is_assignee: false,
                        is_viewer: false,
                        is_subject: false,
                        unassigned_dependent_item: false,
                    };
                    rows.push(EndpointRow {
                        endpoint,
                        relationship,
                        visibility: context.visibility,
                        context,
                        is_self,
                        pat_scope,
                        item_dimension: false,
                        status: expected_status(
                            endpoint,
                            &PolicyContext {
                                item: context,
                                is_self,
                                pat_scope,
                            },
                        ),
                    });
                }
            }
            continue;
        }
        for visibility in Visibility::ALL {
            for relationship in Relationship::ALL {
                let context = AccessContext {
                    caller: match relationship {
                        Relationship::Agent => CallerKind::Agent,
                        Relationship::SuperAdmin => CallerKind::AppMember(Role::SuperAdmin),
                        _ => CallerKind::AppMember(Role::User),
                    },
                    visibility,
                    is_owner: matches!(relationship, Relationship::Owner),
                    is_assignee: matches!(relationship, Relationship::Assignee),
                    is_viewer: matches!(relationship, Relationship::Viewer),
                    is_subject: matches!(relationship, Relationship::Subject),
                    unassigned_dependent_item: false,
                };
                let expanded_dimension = list_dimension
                    || matches!(
                        endpoint.policy,
                        Policy::CustomItem { .. }
                            | Policy::StreamResource {
                                member_scoped: true,
                                ..
                            }
                    );
                let scopes: &[Option<PatScope>] = match (context.caller, expanded_dimension) {
                    (CallerKind::Agent, true) => &[
                        Some(PatScope::Read),
                        Some(PatScope::Write),
                        Some(PatScope::Admin),
                    ],
                    (CallerKind::Agent, false) => &[Some(PatScope::Write)],
                    _ => &[None],
                };
                let selves: &[bool] = if expanded_dimension {
                    &[true, false]
                } else {
                    &[false]
                };
                for &pat_scope in scopes {
                    for &is_self in selves {
                        let status = expected_status(
                            endpoint,
                            &PolicyContext {
                                item: context,
                                is_self,
                                pat_scope,
                            },
                        );
                        rows.push(EndpointRow {
                            endpoint,
                            relationship,
                            visibility,
                            context,
                            is_self,
                            pat_scope,
                            item_dimension: true,
                            status,
                        });
                    }
                }
            }
        }
    }
    // UD=true is possible for five of the seven fixture relationships. An
    // assignee contradicts "unassigned"; a dependent subject cannot log in.
    for mut row in rows.clone() {
        if matches!(row.endpoint.policy, Policy::Item(Capability::Take))
            && !matches!(
                row.relationship,
                Relationship::Assignee | Relationship::Subject
            )
        {
            row.context.unassigned_dependent_item = true;
            row.status = expected_status(
                row.endpoint,
                &PolicyContext {
                    item: row.context,
                    is_self: row.is_self,
                    pat_scope: row.pat_scope,
                },
            );
            rows.push(row);
        }
    }
    rows
}
fn expected_status(endpoint: Endpoint, context: &PolicyContext) -> u16 {
    if let Some(scope) = context.pat_scope {
        let required = if matches!(endpoint.method, "GET" | "HEAD") {
            "read"
        } else {
            "write"
        };
        if !crate::extract::pat_allows_scope(&scope.scopes(), required) {
            return 403;
        }
    }
    endpoint.policy.expected(context)
}
pub const ENDPOINT_MARKER: &str = "\n<!-- HTTP endpoint matrix -->\n";
pub fn permissions_markdown() -> String {
    use std::fmt::Write;
    let mut doc = access::permissions_markdown();
    doc.push_str(ENDPOINT_MARKER);
    doc.push_str("\nAll mutation transports (LAN, Funnel, enrollment and agent socket) return `maintenance` (503, `Retry-After: 1`) before authentication/idempotency while the installer write fence exists. Reads and readiness are unaffected.\n");
    doc.push_str("\n## HTTP endpoint dimension\n\nGenerated from `porchlight_api::permissions`; regenerate both dimensions with `CARGO_NET_OFFLINE=true cargo run --manifest-path server/Cargo.toml -p porchlight-api --example endpoint-matrix`.\n\nDomain rows use live session+mTLS OR a PAT on the agent socket. `/v1/admin/…` rows are agent-only (app callers receive 404); `/v1/feeds…` rows are app-only with separate `/v1/admin/feeds…` counterparts. `/v1/stream` is session+mTLS only (agent rows are 404); SSE results mean allowed tickle or filtered frame, not HTTP statuses. PAT scopes form the hierarchy **admin ⊇ write ⊇ read**: every live token can read; mutations require write or admin; admin-only endpoints require admin. Endpoint-owned pluggable policies describe item, member self-only, list, settings and admin rules. Custom callbacks must explicitly enforce admin PAT scope when required; the generic HTTP-method precheck enforces read/write only. UD records whether an item is unassigned and about a dependent; take requires UD=true even for agents. Additional cases cover request-specific permission branches. App creation sets caller as owner; agents may choose any live household owner. PATCH details/overrides require edit, owner transfer additionally reassign, visibility/viewers change_visibility; status/board/reopen change_status; complete complete. Versioned mutations require If-Match and may return 204 when access is removed. Collection reads omit invisible items. Conflict masking nulls both counterpart ids and replaces its title with Busy. Idempotency binds method, path/query and body, not If-Match; rotate the key after version-conflict.\n\nNon-item rows cross caller kind and PAT scope with **self / other** target members independently of item ACLs. For `/v1/me/settings` the route always resolves the caller's settings; other-target rows are nonapplicable and omitted because there is no target-member parameter. Read PATs cannot mutate; write/admin PATs may patch their owner's member, but cannot request `make_zone_authority:true` (no device). Item rows retain their item ACL dimension and use write-scoped PATs; custom item policies additionally cross independent self/other target members and all PAT scopes. Collaboration rows likewise cross the full item ACL dimension with all PAT scopes and independent resource self/other (author or uploader), not item ownership. The comment-attachment delete case uses comment authorship/moderation rather than the item-editor rule. Notification SSE rows likewise retain the full item relationship/visibility dimension while independently crossing recipient self/other and all PAT scopes; item ACL never grants another member’s notification. List rows retain private/household/selected and owner/viewer/inherited item relationships, independently crossed with self/other and all three PAT scopes; self never changes list ACLs. `GET /v1/lists?include_orphaned=true` admits deleted-parent cleanup for the list owner or admin, but content still requires the parent ACL ignoring only deletion. Other cleaners receive a nameless `redacted: true` descriptor with delete only and empty entries/checks/viewers; stale-delete current uses the same rule.\n\n| Method | Endpoint | Case | Relationship / role | PAT scope | Target member / resource | Visibility | UD | Policy | Result |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for table in endpoint_tables() {
        for row in matrix_rows(table) {
            let cap = row.endpoint.policy.label();
            let result = if row.endpoint.path == "/v1/stream" {
                match row.status {
                    200 => "allowed".to_owned(),
                    204 => "filtered".to_owned(),
                    _ => row.status.to_string(),
                }
            } else {
                row.status.to_string()
            };
            writeln!(
                doc,
                "| {} | `{}` | {} | {} | {} | {} | {} | {} | {} | {} |",
                row.endpoint.method,
                row.endpoint.path,
                row.endpoint.case,
                if !row.item_dimension && matches!(row.relationship, Relationship::Other) {
                    "user"
                } else {
                    row.relationship.as_str()
                },
                row.pat_scope.map(PatScope::as_str).unwrap_or("—"),
                if row.item_dimension
                    && !matches!(
                        row.endpoint.policy,
                        Policy::List { .. }
                            | Policy::ListCollection(_)
                            | Policy::CustomItem { .. }
                            | Policy::ItemResource { .. }
                            | Policy::StreamResource {
                                member_scoped: true,
                                ..
                            }
                    )
                {
                    "—"
                } else if row.is_self {
                    "self"
                } else {
                    "other"
                },
                if row.item_dimension {
                    row.visibility.as_str()
                } else {
                    "—"
                },
                if row.item_dimension {
                    row.context.unassigned_dependent_item.to_string()
                } else {
                    "—".into()
                },
                cap,
                result
            )
            .unwrap();
        }
    }
    doc
}

/// Matrix expectations come from the same core policy as read capabilities.
pub fn read_allowed(caller: &str, visibility: &str, relationship: &str) -> bool {
    use porchlight_core::access::{self, AccessContext, CallerKind};
    access::can_view(&AccessContext {
        caller: if caller == "agent" {
            CallerKind::Agent
        } else {
            CallerKind::AppMember(caller.parse().expect("matrix role"))
        },
        visibility: visibility.parse().expect("matrix visibility"),
        is_owner: relationship == "owner",
        is_assignee: relationship == "assignee",
        is_viewer: relationship == "viewer",
        is_subject: matches!(relationship, "subject" | "subject-only"),
        unassigned_dependent_item: false,
    })
}

#[cfg(test)]
#[path = "permissions_audit_rust.rs"]
mod audit_rust;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn member_self_is_independent_of_item_acl() {
        for mut row in matrix_rows(READ_ENDPOINTS) {
            for is_self in [false, true] {
                row.context.is_owner = !is_self;
                let context = PolicyContext {
                    item: row.context,
                    is_self,
                    pat_scope: row.pat_scope,
                };
                assert_eq!(
                    row.endpoint.policy.expected(&context),
                    if row.endpoint.method == "PATCH"
                        && (!is_self
                            || (row.endpoint.case == "authority"
                                && matches!(row.context.caller, CallerKind::Agent)))
                    {
                        403
                    } else {
                        200
                    }
                );
            }
        }
    }
    #[test]
    fn all_endpoints_are_registered_on_their_surfaces() {
        assert_endpoint_coverage(crate::routes::ROUTES, &endpoint_tables());
    }
    pub(super) fn assert_endpoint_coverage(
        routes: &[crate::routes::Route],
        tables: &[&[Endpoint]],
    ) {
        audit_rust::assert_registered_routes_have_policy(routes, tables);
        for table in tables {
            for e in *table {
                let surfaces = endpoint_surfaces(e);
                for surface in [crate::routes::Surface::Mtls, crate::routes::Surface::Agent] {
                    let registered = routes.iter().any(|r| {
                        r.surface == surface
                            && r.method == e.method
                            && r.path == e.path
                            && r.handler == e.handler
                    });
                    assert_eq!(
                        registered,
                        surfaces.contains(&surface),
                        "{} {} {surface:?}",
                        e.method,
                        e.path
                    );
                }
                if e.path == "/v1/stream" || e.path.starts_with("/v1/backups") {
                    assert!(
                        !routes
                            .iter()
                            .any(|r| r.surface == crate::routes::Surface::Agent && r.path == e.path),
                        "SSE must not be registered on the agent surface"
                    );
                }
                if e.path.starts_with("/v1/feeds") {
                    let path = e.path.replacen("/v1/feeds", "/v1/admin/feeds", 1);
                    let handler = format!("admin_{}", e.handler);
                    assert!(
                        tables
                            .iter()
                            .any(|t| t.iter().any(|other| other.path == path
                                && other.method == e.method
                                && other.handler == handler)),
                        "calendar agent counterpart missing"
                    );
                }
            }
        }
    }
}
