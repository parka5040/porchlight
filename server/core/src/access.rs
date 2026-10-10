//! Item authorization from authoritative relationships only, never from derived agenda rows.
//!
//! The caller must already be authenticated as a live member of the item's household (or an agent).
//! App super admins obey visibility; subjects gain no implicit access. Dependents have no app account.
#![deny(missing_docs)]

use serde::{Deserialize, Serialize, Serializer, ser::SerializeSeq};
use std::str::FromStr;

/// A household member's role, using the database/API strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Full control of visible items in the app.
    SuperAdmin,
    /// An account with enrolled devices.
    User,
    /// A profile without an account or device; never an authorized app caller.
    Dependent,
}

impl Role {
    /// The exact database/API value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SuperAdmin => "super_admin",
            Self::User => "user",
            Self::Dependent => "dependent",
        }
    }
}

/// An unknown role or visibility; invalid authoritative values never silently grant access.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("unknown access value")]
pub struct AccessValueError;

impl FromStr for Role {
    type Err = AccessValueError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "super_admin" => Ok(Self::SuperAdmin),
            "user" => Ok(Self::User),
            "dependent" => Ok(Self::Dependent),
            _ => Err(AccessValueError),
        }
    }
}

/// Authoritative item visibility, using the database/API strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// All live app members of the household.
    Household,
    /// Only the owner and assignees.
    Private,
    /// The owner, assignees and explicit viewers.
    Selected,
}

impl Visibility {
    /// All authoritative visibility values, shared by matrices and wire-schema parity checks.
    pub const ALL: [Self; 3] = [Self::Household, Self::Private, Self::Selected];

    /// The exact database/API value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Household => "household",
            Self::Private => "private",
            Self::Selected => "selected",
        }
    }
}

impl FromStr for Visibility {
    type Err = AccessValueError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "household" => Ok(Self::Household),
            "private" => Ok(Self::Private),
            "selected" => Ok(Self::Selected),
            _ => Err(AccessValueError),
        }
    }
}

/// Authorization channel; agent token scopes remain the HTTP layer's responsibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CallerKind {
    /// An authenticated live app member of the item's household, with their authoritative role.
    AppMember(Role),
    /// An authenticated agent; sees every item and holds every item capability.
    Agent,
}

/// Relationships read from the authoritative item and ACL tables at the point of use.
///
/// The default is a private item with no relationships and grants no access.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct AccessContext {
    /// Authenticated channel and household role.
    pub caller: CallerKind,
    /// Authoritative item visibility.
    pub visibility: Visibility,
    /// Caller is the current owner.
    pub is_owner: bool,
    /// Caller is an assignee.
    pub is_assignee: bool,
    /// Caller is an explicitly selected viewer.
    pub is_viewer: bool,
    /// Caller is the subject; deliberately confers no access or capability.
    pub is_subject: bool,
    /// Subject is a dependent and the item has no assignees (not merely no caller assignment).
    pub unassigned_dependent_item: bool,
}

impl Default for AccessContext {
    fn default() -> Self {
        Self {
            caller: CallerKind::AppMember(Role::User),
            visibility: Visibility::Private,
            is_owner: false,
            is_assignee: false,
            is_viewer: false,
            is_subject: false,
            unassigned_dependent_item: false,
        }
    }
}

/// Stable list actions. Attached lists map to item capabilities; standalone lists have no assignees.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListCapability {
    /// Read list and entries.
    View,
    /// Check/uncheck entries.
    Check,
    /// Manage and archive entries, or clear Got it.
    Manage,
    /// Rename the list.
    Edit,
    /// Transfer standalone ownership.
    Reassign,
    /// Soft-delete the list.
    Delete,
    /// Replace standalone selected viewers.
    ChangeVisibility,
}
impl ListCapability {
    /// Stable response ordering.
    pub const ALL: [Self; 7] = [
        Self::View,
        Self::Check,
        Self::Manage,
        Self::Edit,
        Self::Reassign,
        Self::Delete,
        Self::ChangeVisibility,
    ];
    /// Exact wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Check => "check",
            Self::Manage => "manage",
            Self::Edit => "edit",
            Self::Reassign => "reassign",
            Self::Delete => "delete",
            Self::ChangeVisibility => "change_visibility",
        }
    }
    /// Corresponding authoritative item action for attached lists.
    pub const fn item_capability(self) -> Capability {
        match self {
            Self::View => Capability::View,
            Self::Check => Capability::ChangeStatus,
            Self::Manage | Self::Edit => Capability::Edit,
            Self::Reassign => Capability::Reassign,
            Self::Delete => Capability::Delete,
            Self::ChangeVisibility => Capability::ChangeVisibility,
        }
    }
}
/// Standalone ACL uses the item rule shape without assignee/subject/take relationships.
pub fn standalone_list_context(
    caller: CallerKind,
    visibility: Visibility,
    is_owner: bool,
    is_viewer: bool,
) -> AccessContext {
    AccessContext {
        caller,
        visibility,
        is_owner,
        is_viewer,
        ..Default::default()
    }
}
/// R51: all standalone readers may manipulate entries; metadata remains owner/admin-only.
/// Super admins follow visibility, as they do for items; agents see all.
pub fn standalone_list_can(context: &AccessContext, action: ListCapability) -> bool {
    let caps = capabilities(context);
    match action {
        ListCapability::Check | ListCapability::Manage => caps.contains(Capability::View),
        _ => caps.contains(action.item_capability()),
    }
}
/// Whether an authoritative attached list context permits the action.
pub fn list_can(context: &AccessContext, action: ListCapability) -> bool {
    capabilities(context).contains(action.item_capability())
}

/// Stable item action names, declared in the API's serialization order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Capability {
    /// Read item data.
    View,
    /// Add a comment.
    Comment,
    /// Change Kanban status.
    ChangeStatus,
    /// Complete an occurrence.
    Complete,
    /// Snooze a reminder.
    Snooze,
    /// Edit item details.
    Edit,
    /// Edit reminder definitions.
    EditReminders,
    /// Change assignees or transfer ownership.
    Reassign,
    /// Change visibility or selected viewers.
    ChangeVisibility,
    /// Take an unassigned item whose subject is a dependent.
    Take,
    /// Soft-delete the item.
    Delete,
}

impl Capability {
    /// Every capability in stable API order.
    pub const ALL: [Self; 11] = [
        Self::View,
        Self::Comment,
        Self::ChangeStatus,
        Self::Complete,
        Self::Snooze,
        Self::Edit,
        Self::EditReminders,
        Self::Reassign,
        Self::ChangeVisibility,
        Self::Take,
        Self::Delete,
    ];

    /// The exact stable API string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Comment => "comment",
            Self::ChangeStatus => "change_status",
            Self::Complete => "complete",
            Self::Snooze => "snooze",
            Self::Edit => "edit",
            Self::EditReminders => "edit_reminders",
            Self::Reassign => "reassign",
            Self::ChangeVisibility => "change_visibility",
            Self::Take => "take",
            Self::Delete => "delete",
        }
    }
}

/// Authorized item actions, serialized as an array of stable strings in contract order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapabilitySet(u16);

impl CapabilitySet {
    /// Whether an action is authorized.
    pub const fn contains(self, capability: Capability) -> bool {
        self.0 & (1 << capability as u8) != 0
    }

    /// Whether no item actions are authorized.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Authorized actions in stable API serialization order.
    pub fn iter(self) -> impl Iterator<Item = Capability> {
        Capability::ALL
            .into_iter()
            .filter(move |cap| self.contains(*cap))
    }

    fn insert(&mut self, capability: Capability) {
        self.0 |= 1 << capability as u8;
    }
}

impl Serialize for CapabilitySet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.count_ones() as usize))?;
        for cap in self.iter() {
            seq.serialize_element(cap.as_str())?;
        }
        seq.end()
    }
}

/// Whether the caller can see the item. Subjects never confer implicit access.
pub fn can_view(ctx: &AccessContext) -> bool {
    match ctx.caller {
        CallerKind::Agent => true,
        CallerKind::AppMember(Role::Dependent) => false,
        CallerKind::AppMember(_) => match ctx.visibility {
            Visibility::Household => true,
            Visibility::Private => ctx.is_owner || ctx.is_assignee,
            Visibility::Selected => ctx.is_owner || ctx.is_assignee || ctx.is_viewer,
        },
    }
}

/// Compute item actions. Invisible items yield no capabilities, even to app super admins.
pub fn capabilities(ctx: &AccessContext) -> CapabilitySet {
    use Capability::*;
    let mut set = CapabilitySet::default();
    if ctx.caller == CallerKind::Agent {
        for cap in Capability::ALL {
            set.insert(cap);
        }
        return set;
    }
    if !can_view(ctx) {
        return set;
    }
    set.insert(View);
    set.insert(Comment);
    let full_control = ctx.is_owner || ctx.caller == CallerKind::AppMember(Role::SuperAdmin);
    if full_control || ctx.is_assignee {
        for cap in [ChangeStatus, Complete, Snooze] {
            set.insert(cap);
        }
    }
    if full_control {
        for cap in [Edit, EditReminders, Reassign, ChangeVisibility, Delete] {
            set.insert(cap);
        }
    }
    if ctx.unassigned_dependent_item {
        set.insert(Take);
    }
    set
}

/// One row of the exhaustive core permission matrix (HTTP endpoint dimension is added by S4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionMatrixRow {
    /// Role/channel, visibility and relationship flags tested by this row.
    pub context: AccessContext,
    /// Authorized actions for that context.
    pub capabilities: CapabilitySet,
}

/// Enumerate every role/channel × visibility × combination of the five boolean flags.
///
/// All combinations are included, even logically impossible authoritative states (for example,
/// `is_assignee` with `unassigned_dependent_item`), to show the policy is total and testable.
pub fn permission_matrix() -> Vec<PermissionMatrixRow> {
    let mut rows = Vec::with_capacity(4 * 3 * 32);
    for caller in [
        CallerKind::AppMember(Role::SuperAdmin),
        CallerKind::AppMember(Role::User),
        CallerKind::AppMember(Role::Dependent),
        CallerKind::Agent,
    ] {
        for visibility in Visibility::ALL {
            for flags in 0..32 {
                let context = AccessContext {
                    caller,
                    visibility,
                    is_owner: flags & 1 != 0,
                    is_assignee: flags & 2 != 0,
                    is_viewer: flags & 4 != 0,
                    is_subject: flags & 8 != 0,
                    unassigned_dependent_item: flags & 16 != 0,
                };
                rows.push(PermissionMatrixRow {
                    context,
                    capabilities: capabilities(&context),
                });
            }
        }
    }
    rows
}

/// Deterministic Markdown for the generated core permissions document.
pub fn permissions_markdown() -> String {
    use std::fmt::Write;
    let mut doc = String::from(
        "# Permissions matrix\n\n\
Generated from `porchlight-core::access`; do not edit by hand. From the repository root, regenerate with:\n\n\
```sh\nCARGO_NET_OFFLINE=true cargo run --manifest-path server/Cargo.toml -p porchlight-core --example permissions-matrix\n```\n\n\
The caller is already authenticated as a live member of the item's household or an agent.\n\
Subjects gain no implicit access; app super admins obey visibility; dependents cannot call the app.\n\
Agents see everything and hold all item capabilities (agent token scopes are enforced separately).\n\
`UD` means the subject is a dependent and there are **no assignees**. Every combination of flags\n\
is listed, including impossible states such as an assignee on an unassigned item, to test a total policy.\n\
Capability arrays use the stable API order. The HTTP endpoint dimension is added by S4.\n\n\
| Caller | Visibility | Owner | Assignee | Viewer | Subject | UD | Capabilities |\n\
| --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for row in permission_matrix() {
        let ctx = row.context;
        let caller = match ctx.caller {
            CallerKind::Agent => "agent",
            CallerKind::AppMember(role) => role.as_str(),
        };
        let flag = |value| if value { "yes" } else { "no" };
        let caps = row
            .capabilities
            .iter()
            .map(Capability::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            doc,
            "| {caller} | {} | {} | {} | {} | {} | {} | {} |",
            ctx.visibility.as_str(),
            flag(ctx.is_owner),
            flag(ctx.is_assignee),
            flag(ctx.is_viewer),
            flag(ctx.is_subject),
            flag(ctx.unassigned_dependent_item),
            if caps.is_empty() { "—" } else { &caps }
        )
        .expect("writing to a String cannot fail");
    }
    doc
}
