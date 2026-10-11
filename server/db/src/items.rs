//! In-transaction authoritative writes. No permission checks: S4 owns caller ACLs.
//! Call before acquiring any domain locks; rollback (or use a savepoint) on any error.
use crate::derive::{self, DeriveOutcome, Error, ItemRow, Result};
use chrono::{DateTime, Utc};
use porchlight_core::{items as core, ordering};
use serde::{Deserialize, Serialize};
use sqlx::{Acquire, PgConnection, Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemWrite {
    pub title: String,
    pub notes: String,
    pub priority: i16,
    pub owner_id: Uuid,
    pub subject_member_id: Option<Uuid>,
    pub visibility: String,
    pub address: Option<String>,
    pub source: Option<String>,
    pub buffer_before_min: i32,
    pub buffer_after_min: i32,
    pub est_minutes: Option<i32>,
    #[serde(flatten)]
    pub timing: core::SeriesInput,
}
impl Default for ItemWrite {
    fn default() -> Self {
        Self {
            title: String::new(),
            notes: String::new(),
            priority: 0,
            owner_id: Uuid::nil(),
            subject_member_id: None,
            visibility: "household".into(),
            address: None,
            source: None,
            buffer_before_min: 0,
            buffer_after_min: 0,
            est_minutes: None,
            timing: Default::default(),
        }
    }
}
impl From<&ItemRow> for ItemWrite {
    fn from(r: &ItemRow) -> Self {
        Self {
            title: r.title.clone(),
            notes: r.notes.clone(),
            priority: r.priority,
            owner_id: r.owner_id,
            subject_member_id: r.subject_member_id,
            visibility: r.visibility.clone(),
            address: r.address.clone(),
            source: r.source.clone(),
            buffer_before_min: r.buffer_before_min,
            buffer_after_min: r.buffer_after_min,
            est_minutes: r.est_minutes,
            timing: r.timing.clone(),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Relationships {
    pub assignees: Vec<Uuid>,
    pub viewers: Vec<Uuid>,
    pub reminder_offsets: Vec<i32>,
}
#[derive(Clone, Debug)]
pub struct WriteOutcome {
    pub item_id: Uuid,
    pub derived: DeriveOutcome,
}
#[derive(Clone, Debug)]
pub struct SplitOutcome {
    pub head_id: Option<Uuid>,
    pub tail_id: Uuid,
    pub derived: DeriveOutcome,
}

mod scope;
pub use scope::{BoardMove, ScopeRequest, WriteScope, agenda_members};

fn write_members(fields: &ItemWrite, sets: &Relationships) -> Vec<Uuid> {
    porchlight_core::agenda::agenda_members(
        fields.owner_id,
        &sets.assignees,
        fields
            .subject_member_id
            .map(|id| (id, porchlight_core::access::Role::Dependent)),
    )
    .into_iter()
    .collect()
}
fn data(fields: &ItemWrite, spec: &core::SeriesSpec) -> Result<serde_json::Value> {
    let mut v = serde_json::to_value(fields).expect("item fields serialize");
    let interval = spec.input().completion_interval.map(|i| {
        format!(
            "{} months {} days {} microseconds",
            i.months, i.days, i.microseconds
        )
    });
    v["tzid"] = serde_json::to_value(&spec.input().tzid).unwrap();
    v["completion_interval"] = serde_json::to_value(interval).unwrap();
    let cursor = spec.calendar_cursor();
    v["nominal_local_anchor"] =
        serde_json::to_value(cursor.map(|c| c.nominal_local_anchor)).unwrap();
    let duration = cursor.and_then(|c| c.wall_duration);
    let micros = duration
        .map(|d| {
            d.num_microseconds()
                .filter(|us| chrono::Duration::microseconds(*us) == d)
                .ok_or_else(|| core::ValidationError {
                    field: "nominal_wall_duration",
                    reason: "duration is not exactly representable as microseconds".into(),
                })
        })
        .transpose()?;
    v["nominal_wall_duration"] = serde_json::to_value(micros).unwrap();
    Ok(v)
}
const INSERT: &str = "insert into porchlight.items(id,household_id,title,notes,priority,owner_id,subject_member_id,visibility,address,source,buffer_before_min,buffer_after_min,est_minutes,kind,time_mode,tzid,all_day,due_date,due_at,starts_at,ends_at,local_due,local_start,local_end,rrule,recurrence_mode,completion_interval,missed_policy,nominal_local_anchor,nominal_wall_duration,series_id,split_from_item_id,split_from_seq,ics_source_id,reminder_origin_id) select $1,$2,r.title,r.notes,r.priority,r.owner_id,r.subject_member_id,r.visibility,r.address,r.source,r.buffer_before_min,r.buffer_after_min,r.est_minutes,r.kind,r.time_mode,r.tzid,r.all_day,r.due_date,r.due_at,r.starts_at,r.ends_at,r.local_due,r.local_start,r.local_end,r.rrule,r.recurrence_mode,r.completion_interval,r.missed_policy,r.nominal_local_anchor,r.nominal_wall_duration,$4,$5,$6,r.ics_source_id,(select coalesce(p.reminder_origin_id,p.id) from porchlight.items p where p.id=$5 and p.household_id=$2) from jsonb_populate_record(null::porchlight.items,$3) r";
const UPDATE: &str = "update porchlight.items i set (title,notes,priority,owner_id,subject_member_id,visibility,address,source,buffer_before_min,buffer_after_min,est_minutes,kind,time_mode,tzid,all_day,due_date,due_at,starts_at,ends_at,local_due,local_start,local_end,rrule,recurrence_mode,completion_interval,missed_policy,nominal_local_anchor,nominal_wall_duration)=(r.title,r.notes,r.priority,r.owner_id,r.subject_member_id,r.visibility,r.address,r.source,r.buffer_before_min,r.buffer_after_min,r.est_minutes,r.kind,r.time_mode,r.tzid,r.all_day,r.due_date,r.due_at,r.starts_at,r.ends_at,r.local_due,r.local_start,r.local_end,r.rrule,r.recurrence_mode,r.completion_interval,r.missed_policy,r.nominal_local_anchor,r.nominal_wall_duration) from jsonb_populate_record(null::porchlight.items,$2) r where i.id=$1";
async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    fields: &ItemWrite,
    spec: &core::SeriesSpec,
    lineage: (Option<Uuid>, Option<Uuid>, Option<i32>),
) -> Result<Uuid> {
    insert_calendar(tx, household, fields, spec, lineage, None).await
}
async fn insert_calendar(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    fields: &ItemWrite,
    spec: &core::SeriesSpec,
    lineage: (Option<Uuid>, Option<Uuid>, Option<i32>),
    source: Option<Uuid>,
) -> Result<Uuid> {
    let id = Uuid::now_v7();
    let mut payload = data(fields, spec)?;
    payload["ics_source_id"] = serde_json::to_value(source).expect("UUID serializes");
    sqlx::query(sqlx::AssertSqlSafe(INSERT))
        .bind(id)
        .bind(household)
        .bind(payload)
        .bind(lineage.0)
        .bind(lineage.1)
        .bind(lineage.2)
        .execute(&mut **tx)
        .await?;
    Ok(id)
}
async fn persist(
    conn: &mut PgConnection,
    id: Uuid,
    fields: &ItemWrite,
    spec: &core::SeriesSpec,
) -> Result<()> {
    sqlx::query(sqlx::AssertSqlSafe(UPDATE))
        .bind(id)
        .bind(data(fields, spec)?)
        .execute(conn)
        .await?;
    Ok(())
}
// All replacement paths preserve unchanged children: deleting/reinserting an
// existing assignee manufactures immutable assignment events/notifications, and
// recreating reminders unnecessarily changes their identities.
async fn restore_relationships(
    conn: &mut PgConnection,
    household: Uuid,
    id: Uuid,
    sets: &Relationships,
) -> Result<()> {
    sqlx::query(
        "delete from porchlight.item_assignees where item_id=$1 and not(member_id=any($2))",
    )
    .bind(id)
    .bind(&sets.assignees)
    .execute(&mut *conn)
    .await?;
    sqlx::query("delete from porchlight.item_viewers where item_id=$1 and not(member_id=any($2))")
        .bind(id)
        .bind(&sets.viewers)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "delete from porchlight.item_reminders where item_id=$1 and not(offset_minutes=any($2))",
    )
    .bind(id)
    .bind(&sets.reminder_offsets)
    .execute(&mut *conn)
    .await?;
    create_relationships(conn, household, id, sets).await
}
async fn create_relationships(
    conn: &mut PgConnection,
    household: Uuid,
    id: Uuid,
    sets: &Relationships,
) -> Result<()> {
    if !sets.assignees.is_empty() {
        sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) select $1,$2,unnest($3::uuid[]) on conflict do nothing")
            .bind(household).bind(id).bind(&sets.assignees).execute(&mut *conn).await?;
    }
    if !sets.viewers.is_empty() {
        sqlx::query("insert into porchlight.item_viewers(household_id,item_id,member_id) select $1,$2,unnest($3::uuid[]) on conflict do nothing")
            .bind(household).bind(id).bind(&sets.viewers).execute(&mut *conn).await?;
    }
    if !sets.reminder_offsets.is_empty() {
        sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) select $1,$2,unnest($3::int[]) on conflict do nothing")
            .bind(household).bind(id).bind(&sets.reminder_offsets).execute(&mut *conn).await?;
    }
    Ok(())
}
pub async fn relationships(conn: &mut PgConnection, id: Uuid) -> Result<Relationships> {
    Ok(Relationships{assignees:sqlx::query_scalar("select member_id from porchlight.item_assignees where item_id=$1 order by member_id").bind(id).fetch_all(&mut *conn).await?,viewers:sqlx::query_scalar("select member_id from porchlight.item_viewers where item_id=$1 order by member_id").bind(id).fetch_all(&mut *conn).await?,reminder_offsets:sqlx::query_scalar("select offset_minutes from porchlight.item_reminders where item_id=$1 order by offset_minutes").bind(id).fetch_all(conn).await?})
}
fn changes_timing(a: &core::SeriesInput, b: &core::SeriesInput) -> bool {
    let mut comparable = a.clone();
    comparable.missed_policy = b.missed_policy;
    comparable.completion_interval = b.completion_interval;
    comparable != *b
}
// Compatibility single-operation wrappers. Compose ONLY WriteScope methods, not wrappers.
fn outcome(mut rows: Vec<WriteOutcome>, id: Uuid) -> Result<WriteOutcome> {
    let index = rows
        .iter()
        .position(|r| r.item_id == id)
        .ok_or(Error::NotFound)?;
    Ok(rows.swap_remove(index))
}
/// The same R34 board projection used by reads and domain move planning.
pub async fn is_board_card(
    conn: &mut PgConnection,
    household: Uuid,
    occurrence: Uuid,
    now: DateTime<Utc>,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "select exists(select 1 from porchlight.pl_board_cards($1,$2) where id=$3)",
    )
    .bind(household)
    .bind(now)
    .bind(occurrence)
    .fetch_one(conn)
    .await?)
}
pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    fields: &ItemWrite,
    sets: &Relationships,
    now: DateTime<Utc>,
) -> Result<WriteOutcome> {
    let mut scope = WriteScope::begin_at(
        tx,
        household,
        ScopeRequest {
            members: write_members(fields, sets),
            ..Default::default()
        },
        now,
    )
    .await?;
    let id = scope.create(fields, sets).await?;
    outcome(scope.finish(now).await?, id)
}
pub async fn update(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    id: Uuid,
    version: i32,
    fields: &ItemWrite,
    now: DateTime<Utc>,
) -> Result<WriteOutcome> {
    let sets = relationships(tx, id).await?;
    let mut scope = WriteScope::begin_at(
        tx,
        household,
        ScopeRequest {
            items: vec![id],
            members: write_members(fields, &sets),
            ..Default::default()
        },
        now,
    )
    .await?;
    scope.update(id, version, fields).await?;
    outcome(scope.finish(now).await?, id)
}
pub async fn soft_delete(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    id: Uuid,
    version: i32,
    now: DateTime<Utc>,
) -> Result<WriteOutcome> {
    let mut scope = WriteScope::begin_at(
        tx,
        household,
        ScopeRequest {
            items: vec![id],
            ..Default::default()
        },
        now,
    )
    .await?;
    scope.soft_delete(id, version, now).await?;
    outcome(scope.finish(now).await?, id)
}
pub async fn replace_relationships(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    id: Uuid,
    version: i32,
    sets: &Relationships,
    now: DateTime<Utc>,
) -> Result<WriteOutcome> {
    let row = derive::load_item(tx, id).await?;
    let mut scope = WriteScope::begin_at(
        tx,
        household,
        ScopeRequest {
            items: vec![id],
            members: write_members(&ItemWrite::from(&row), sets),
            ..Default::default()
        },
        now,
    )
    .await?;
    scope.replace_relationships(id, version, sets).await?;
    outcome(scope.finish(now).await?, id)
}
pub async fn set_status(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    occ: Uuid,
    version: i32,
    status: core::Status,
    completed_by: Option<Uuid>,
    now: DateTime<Utc>,
) -> Result<WriteOutcome> {
    let mut scope = WriteScope::begin_at(
        tx,
        household,
        ScopeRequest {
            occurrences: vec![occ],
            reopens: if status.is_open() {
                vec![occ]
            } else {
                Vec::new()
            },
            ..Default::default()
        },
        now,
    )
    .await?;
    let id = scope.occurrence(occ).await?.item_id;
    scope
        .set_status(occ, version, status, completed_by, now)
        .await?;
    outcome(scope.finish(now).await?, id)
}
pub async fn override_occurrence(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    occ: Uuid,
    version: i32,
    replacement: &core::OccurrenceAnchor,
    now: DateTime<Utc>,
) -> Result<WriteOutcome> {
    let mut scope = WriteScope::begin_at(
        tx,
        household,
        ScopeRequest {
            occurrences: vec![occ],
            ..Default::default()
        },
        now,
    )
    .await?;
    let id = scope.occurrence(occ).await?.item_id;
    scope.override_occurrence(occ, version, replacement).await?;
    outcome(scope.finish(now).await?, id)
}
pub async fn move_on_board(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    occ: Uuid,
    version: i32,
    before: Option<Uuid>,
    after: Option<Uuid>,
    now: DateTime<Utc>,
) -> Result<WriteOutcome> {
    let status: String = sqlx::query_scalar(
        "select status from porchlight.item_occurrences where household_id=$1 and id=$2",
    )
    .bind(household)
    .bind(occ)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::NotFound)?;
    let status = status.parse().map_err(|_| Error::NotFound)?;
    let mut scope = WriteScope::begin_at(
        tx,
        household,
        ScopeRequest {
            board_moves: vec![BoardMove {
                occurrence: occ,
                before,
                after,
                status,
            }],
            ..Default::default()
        },
        now,
    )
    .await?;
    let id = scope.occurrence(occ).await?.item_id;
    scope.move_on_board(occ, version, before, after).await?;
    outcome(scope.finish(now).await?, id)
}
pub async fn split(
    tx: &mut Transaction<'_, Postgres>,
    household: Uuid,
    id: Uuid,
    version: i32,
    at_seq: u32,
    now: DateTime<Utc>,
) -> Result<SplitOutcome> {
    let mut scope = WriteScope::begin_at(
        tx,
        household,
        ScopeRequest {
            items: vec![id],
            splits: if at_seq == 0 {
                Vec::new()
            } else {
                vec![(id, at_seq)]
            },
            ..Default::default()
        },
        now,
    )
    .await?;
    let (head_id, tail_id) = scope.split(id, version, at_seq).await?;
    let tail = outcome(scope.finish(now).await?, tail_id)?;
    Ok(SplitOutcome {
        head_id,
        tail_id,
        derived: tail.derived,
    })
}
