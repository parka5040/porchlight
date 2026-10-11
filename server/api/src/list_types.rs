//! List wire DTOs shared by the domain service and HTTP adapter.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ListMode {
    Persistent,
    PerOccurrence,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ListVisibility {
    Household,
    Private,
    Selected,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct ListRow {
    pub id: Uuid,
    pub household_id: Uuid,
    pub item_id: Option<Uuid>,
    pub owner_id: Uuid,
    pub name: String,
    pub mode: ListMode,
    pub visibility: ListVisibility,
    pub version: i32,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct EntryRow {
    pub id: Uuid,
    pub household_id: Uuid,
    pub list_id: Uuid,
    pub label: String,
    pub qty: Option<f64>,
    pub unit: String,
    pub url: Option<String>,
    pub notes: String,
    pub position: String,
    pub archived_at: Option<DateTime<Utc>>,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, ToSchema)]
pub struct EntryResponse {
    #[serde(flatten)]
    pub fields: EntryRow,
    pub checked_by: Option<Uuid>,
    pub checked_at: Option<DateTime<Utc>>,
    pub occurrence_id: Option<Uuid>,
}
#[derive(Serialize, ToSchema)]
pub struct ListFields {
    pub id: Uuid,
    pub item_id: Option<Uuid>,
    pub owner_id: Uuid,
    pub mode: ListMode,
    pub version: i32,
    /// Present only on cleanup descriptors whose parent ACL denies content access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub household_id: Option<Uuid>,
    /// Absent on redacted cleanup descriptors; clients display "Private list".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<ListVisibility>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<Option<DateTime<Utc>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}
#[derive(Serialize, ToSchema)]
pub struct ListResponse {
    #[serde(flatten)]
    pub fields: ListFields,
    pub viewers: Vec<Uuid>,
    pub capabilities: Vec<String>,
    pub entries: Vec<EntryResponse>,
    pub got_it: Vec<EntryResponse>,
    pub occurrence_id: Option<Uuid>,
}
impl ListResponse {
    pub fn cleanup(r: ListRow) -> Self {
        Self {
            fields: ListFields {
                id: r.id,
                item_id: r.item_id,
                owner_id: r.owner_id,
                mode: r.mode,
                version: r.version,
                redacted: Some(true),
                household_id: None,
                name: None,
                visibility: None,
                deleted_at: None,
                created_at: None,
                updated_at: None,
            },
            viewers: vec![],
            capabilities: vec!["delete".into()],
            entries: vec![],
            got_it: vec![],
            occurrence_id: None,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub struct ListsPage {
    pub items: Vec<ListResponse>,
    pub next_cursor: Option<String>,
}
#[derive(Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ListsQuery {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
    pub item_id: Option<Uuid>,
    /// Include deleted-parent lists owned by the caller (or app super admin / admin agent) for cleanup.
    /// Content requires the parent's retained ACL; otherwise only a redacted delete descriptor is returned.
    #[serde(default)]
    pub include_orphaned: bool,
}
#[derive(Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    pub occurrence_id: Option<Uuid>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateList {
    pub name: String,
    pub item_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub mode: Option<ListMode>,
    pub visibility: Option<ListVisibility>,
    pub viewers: Option<Vec<Uuid>>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchList {
    pub name: Option<String>,
    pub owner_id: Option<Uuid>,
    pub visibility: Option<ListVisibility>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EntryWrite {
    pub label: Option<String>,
    pub qty: Option<f64>,
    pub unit: Option<String>,
    pub url: Option<String>,
    pub notes: Option<String>,
    pub after_id: Option<Uuid>,
    pub before_id: Option<Uuid>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckRequest {
    pub occurrence_id: Option<Uuid>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewersRequest {
    pub viewers: Vec<Uuid>,
}
