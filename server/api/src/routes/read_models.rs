//! Wire schemas for S4b read projections. Runtime JSON is assembled by the ACL-filtered SQL.
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use uuid::Uuid;
#[derive(utoipa::ToSchema)]
pub struct IntervalRead {
    pub months: i32,
    pub days: i32,
    pub microseconds: i64,
}
#[derive(utoipa::ToSchema)]
pub struct ReminderRead {
    pub id: Uuid,
    pub household_id: Uuid,
    pub item_id: Uuid,
    pub offset_minutes: i32,
    pub created_at: DateTime<Utc>,
}
#[derive(utoipa::ToSchema)]
pub struct ItemRead {
    pub id: Uuid,
    pub household_id: Uuid,
    pub title: String,
    pub notes: String,
    pub priority: i16,
    pub owner_id: Uuid,
    pub subject_member_id: Option<Uuid>,
    pub visibility: String,
    pub source: Option<String>,
    pub ics_source_id: Option<Uuid>,
    pub address: Option<String>,
    pub kind: String,
    pub time_mode: String,
    pub tzid: Option<String>,
    pub all_day: bool,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_due: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_start: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_end: Option<NaiveDateTime>,
    pub buffer_before_min: i32,
    pub buffer_after_min: i32,
    pub est_minutes: Option<i32>,
    pub rrule: Option<String>,
    pub recurrence_mode: Option<String>,
    pub completion_interval: Option<IntervalRead>,
    pub missed_policy: String,
    pub series_id: Option<Uuid>,
    pub split_from_item_id: Option<Uuid>,
    pub split_from_seq: Option<i32>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub nominal_local_anchor: Option<NaiveDateTime>,
    pub nominal_wall_duration: Option<i64>,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub assignees: Vec<Uuid>,
    pub viewers: Vec<Uuid>,
    pub reminders: Vec<ReminderRead>,
    pub capabilities: Vec<String>,
    pub pending_derivation: bool,
    pub occurrence: Option<OccurrenceRead>,
    pub agenda: Option<AgendaRead>,
    pub conflicts: Option<Vec<ConflictRead>>,
    pub entity_type: Option<String>,
}
#[derive(utoipa::ToSchema)]
pub struct OccurrenceRead {
    pub id: Uuid,
    pub household_id: Uuid,
    pub item_id: Uuid,
    pub seq: i32,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_start: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_end: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_due: Option<NaiveDateTime>,
    pub due_date: Option<NaiveDate>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub due_at: Option<DateTime<Utc>>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub override_local_start: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub override_local_end: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub override_local_due: Option<NaiveDateTime>,
    pub override_due_date: Option<NaiveDate>,
    pub is_detached: bool,
    pub disposition: String,
    pub status: String,
    pub board_position: String,
    pub completed_by: Option<Uuid>,
    pub completed_at: Option<DateTime<Utc>>,
    pub skipped_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub board_moved: bool,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(utoipa::ToSchema)]
pub struct AgendaRead {
    /// Recipient-local missed-policy disposition; absent on pre-upgrade projections.
    pub disposition: Option<String>,
    pub household_id: Uuid,
    pub member_id: Uuid,
    pub occurrence_id: Uuid,
    pub item_id: Uuid,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub due_at: Option<DateTime<Utc>>,
    pub due_date: Option<NaiveDate>,
    pub derived_at: DateTime<Utc>,
}
#[derive(utoipa::ToSchema)]
pub struct ConflictRead {
    pub member_id: Uuid,
    pub occurrence_id: Uuid,
    pub other_occurrence_id: Option<Uuid>,
    pub other_item_id: Option<Uuid>,
    pub other_title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub kind: String,
}
#[derive(utoipa::ToSchema)]
pub struct MemberRead {
    pub pending_derivation: bool,
    pub id: Uuid,
    pub household_id: Uuid,
    pub display_name: String,
    pub role: String,
    pub color: String,
    pub initials: String,
    pub tzid: String,
    pub tz_source: String,
    pub tz_authority_device_id: Option<Uuid>,
    pub version: i32,
}
#[derive(utoipa::ToSchema)]
pub struct SettingsRead {
    pub member_id: Uuid,
    pub version: i32,
    pub settings: super::members::TodaySettings,
}
#[derive(utoipa::ToSchema)]
pub struct ScheduledReminderRead {
    pub id: Uuid,
    pub household_id: Uuid,
    pub member_id: Uuid,
    pub occurrence_id: Uuid,
    pub item_id: Uuid,
    pub reminder_id: Uuid,
    pub fire_at: DateTime<Utc>,
    pub derived_at: DateTime<Utc>,
}
#[derive(utoipa::ToSchema)]
pub struct ItemReadPage {
    pub items: Vec<ItemRead>,
    pub next_cursor: Option<String>,
}
#[derive(utoipa::ToSchema)]
pub struct MemberReadPage {
    pub items: Vec<MemberRead>,
    pub next_cursor: Option<String>,
}
#[derive(utoipa::ToSchema)]
pub struct SnapshotRead {
    pub notifications: Vec<super::notifications::Notification>,
    pub notification_prefs: super::notifications::PrefsRead,

    pub comments: Vec<crate::collaboration::Comment>,
    pub attachments: Vec<crate::blobs::Attachment>,
    pub ics_sources: Vec<crate::calendar::SourceSnapshot>,
    pub availability: Vec<super::planning::AvailabilityResponse>,
    pub suggestions: Vec<super::planning::SuggestionResponse>,
    pub members: Vec<MemberRead>,
    pub items: Vec<ItemRead>,
    pub lists: Vec<crate::list_types::ListResponse>,
    pub occurrences: Vec<OccurrenceRead>,
    pub scheduled_reminders: Vec<ScheduledReminderRead>,
    pub member_settings: SettingsRead,
    pub cursor: i64,
}

#[derive(utoipa::ToSchema)]
pub struct SearchListRead {
    #[serde(flatten)]
    pub list: crate::list_types::ListResponse,
    pub entity_type: String,
}
#[derive(utoipa::ToSchema)]
pub struct SearchCommentRead {
    #[serde(flatten)]
    pub comment: crate::collaboration::Comment,
    pub entity_type: String,
}
#[derive(utoipa::ToSchema)]
#[serde(untagged)]
pub enum SearchRead {
    Item(Box<ItemRead>),
    List(Box<SearchListRead>),
    Comment(Box<SearchCommentRead>),
}
#[derive(utoipa::ToSchema)]
pub struct SearchReadPage {
    pub items: Vec<SearchRead>,
    pub next_cursor: Option<String>,
}
