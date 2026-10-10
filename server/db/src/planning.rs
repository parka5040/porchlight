//! Planning projections. Callers serialize on the member AFTER any item scope.
use crate::derive::{self, Result};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc, Weekday};
use porchlight_core::{schedule as core, time};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
type LoadWarningRow = (Uuid, Uuid, Uuid, DateTime<Utc>, DateTime<Utc>);
type TaskRow = (Uuid, Option<DateTime<Utc>>, Option<NaiveDate>, i32, i16);

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Window {
    pub weekday: i16,
    pub starts_local: NaiveTime,
    pub ends_local: NaiveTime,
}
impl Window {
    pub fn core(&self) -> core::AvailabilityWindow {
        core::AvailabilityWindow {
            weekday: [
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
                Weekday::Sat,
                Weekday::Sun,
            ][self.weekday as usize],
            start: self.starts_local,
            end: self.ends_local,
        }
    }
}
pub async fn windows(conn: &mut PgConnection, member: Uuid) -> Result<Vec<Window>> {
    Ok(sqlx::query_as("select weekday,starts_local,ends_local from porchlight.availability_windows where member_id=$1 order by weekday,starts_local,ends_local").bind(member).fetch_all(conn).await?)
}
pub async fn member_schedule(
    conn: &mut PgConnection,
    household: Uuid,
    member: Uuid,
) -> Result<core::MemberSchedule> {
    let zone: String = sqlx::query_scalar("select case when m.role='dependent' then h.tzid else m.tzid end from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=$1 and m.household_id=$2 and m.deleted_at is null and h.deleted_at is null").bind(member).bind(household).fetch_optional(&mut *conn).await?.ok_or(derive::Error::NotFound)?;
    let windows = windows(conn, member)
        .await?
        .iter()
        .map(Window::core)
        .collect();
    let ranges: Vec<(DateTime<Utc>,DateTime<Utc>)> = sqlx::query_as("select lower(a.busy),upper(a.busy) from porchlight.member_agenda a join porchlight.items i on i.id=a.item_id join porchlight.item_derivation d on d.item_id=i.id where a.member_id=$1 and i.deleted_at is null and a.busy is not null").bind(member).fetch_all(conn).await?;
    Ok(core::MemberSchedule {
        member_id: member,
        tz: derive::parse_zone(&zone)?,
        windows,
        busy_ranges: ranges
            .into_iter()
            .map(|(start, end)| core::TimeRange { start, end })
            .collect(),
    })
}
#[derive(sqlx::FromRow)]
struct LoadMember {
    member_id: Uuid,
    household_id: Uuid,
    zone: String,
    windows: sqlx::types::Json<Vec<Window>>,
    old_conflicts: Vec<NaiveDate>,
    pending: bool,
    seen: Option<i64>,
    deleted: bool,
}
#[derive(Serialize)]
struct LoadRow {
    member_id: Uuid,
    date: NaiveDate,
    planned_minutes: i64,
    available_minutes: i64,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
}

/// No new domain locks here: used under the retained member lock by scope/worker.
pub async fn recompute_load(
    conn: &mut PgConnection,
    member: Uuid,
    now: DateTime<Utc>,
) -> Result<()> {
    recompute_load_many(conn, &[member], now).await
}

/// Batch all affected members and dates; do not load busy ranges (load measures
/// total availability, not free slots). Retains S3's bounded derivation SQL footprint.
/// Caller must hold every member lock, after all item/occurrence/job acquisition.
pub async fn recompute_load_many(
    conn: &mut PgConnection,
    members: &[Uuid],
    now: DateTime<Utc>,
) -> Result<()> {
    recompute_load_inner(conn, members, None, now).await
}
/// Recompute only changed member-local dates; pending availability jobs force a full refresh.
pub async fn recompute_load_dates(
    conn: &mut PgConnection,
    members: &[Uuid],
    dates: &[(Uuid, NaiveDate)],
    now: DateTime<Utc>,
) -> Result<()> {
    let dates: Vec<_> = dates
        .iter()
        .filter(|(m, _)| members.contains(m))
        .map(|(m, d)| serde_json::json!({"member_id":m,"date":d}))
        .collect();
    if dates.is_empty() {
        return Ok(());
    }
    recompute_load_inner(conn, members, Some(dates), now).await
}
pub async fn enqueue_unheld_load(conn: &mut PgConnection, members: &[Uuid]) -> Result<()> {
    if !members.is_empty() {
        sqlx::query("select porchlight.pl_signal_member_load($1)")
            .bind(members)
            .execute(conn)
            .await?;
    }
    Ok(())
}
async fn recompute_load_inner(
    conn: &mut PgConnection,
    members: &[Uuid],
    dates: Option<Vec<serde_json::Value>>,
    now: DateTime<Utc>,
) -> Result<()> {
    if members.is_empty() {
        return Ok(());
    }
    let profiles: Vec<LoadMember> = sqlx::query_as("select m.id as member_id,m.household_id,case when m.role='dependent' then h.tzid else m.tzid end as zone,coalesce((select jsonb_agg(jsonb_build_object('weekday',w.weekday,'starts_local',w.starts_local,'ends_local',w.ends_local)) from porchlight.availability_windows w where w.member_id=m.id),'[]'::jsonb) as windows,array(select l.date from porchlight.member_daily_load l where l.member_id=m.id and l.planned_minutes>l.available_minutes) as old_conflicts,coalesce(j.requested_gen>j.applied_gen,false) as pending,j.requested_gen as seen,(m.deleted_at is not null or h.deleted_at is not null) as deleted from porchlight.household_members m join porchlight.households h on h.id=m.household_id left join porchlight.member_load_jobs j on j.member_id=m.id where m.id=any($1) order by m.id").bind(members).fetch_all(&mut *conn).await?;
    // Scheduled start day wins over the deadline, with the member's effective zone.
    let full: Vec<_> = members
        .iter()
        .copied()
        .filter(|m| {
            dates.is_none()
                || profiles
                    .iter()
                    .find(|p| p.member_id == *m)
                    .is_none_or(|p| p.pending)
        })
        .collect();
    let selected = dates.unwrap_or_default();
    // Both SQL membership predicates below are uncorrelated IN sets (hashed
    // once, not a JSON expansion per agenda/projection row). Mirror that here.
    let selected_set: BTreeSet<(Uuid, NaiveDate)> = selected
        .iter()
        .map(|r| {
            (
                serde_json::from_value(r["member_id"].clone()).expect("internal member date"),
                serde_json::from_value(r["date"].clone()).expect("internal member date"),
            )
        })
        .collect();
    let tasks: Vec<(Uuid, NaiveDate, i64)> = sqlx::query_as("select a.member_id,coalesce((a.starts_at at time zone case when m.role='dependent' then h.tzid else m.tzid end)::date,(a.due_at at time zone case when m.role='dependent' then h.tzid else m.tzid end)::date,a.due_date) as date,sum(i.est_minutes)::bigint from porchlight.member_agenda a join porchlight.items i on i.id=a.item_id join porchlight.item_occurrences o on o.id=a.occurrence_id join porchlight.item_derivation d on d.item_id=i.id join porchlight.household_members m on m.id=a.member_id join porchlight.households h on h.id=m.household_id where m.deleted_at is null and h.deleted_at is null and a.member_id=any($1) and (a.member_id=any($2) or (a.member_id,coalesce((a.starts_at at time zone case when m.role='dependent' then h.tzid else m.tzid end)::date,(a.due_at at time zone case when m.role='dependent' then h.tzid else m.tzid end)::date,a.due_date)) in(select r.member_id,r.date from jsonb_to_recordset($3) as r(member_id uuid,date date))) and i.deleted_at is null and i.kind in ('todo','deadline') and i.est_minutes is not null and o.status in ('backlog','todo','doing') and o.disposition<>'expired' and d.requested_gen=d.applied_gen group by 1,2 having coalesce((a.starts_at at time zone case when m.role='dependent' then h.tzid else m.tzid end)::date,(a.due_at at time zone case when m.role='dependent' then h.tzid else m.tzid end)::date,a.due_date) is not null").bind(members).bind(&full).bind(sqlx::types::Json(&selected)).fetch_all(&mut *conn).await?;
    let mut by_member: BTreeMap<Uuid, BTreeMap<NaiveDate, u64>> = BTreeMap::new();
    for (member, date, minutes) in tasks {
        by_member
            .entry(member)
            .or_default()
            .insert(date, minutes as u64);
    }
    let mut rows = Vec::new();
    let mut transitions = Vec::new();
    for profile in &profiles {
        if profile.deleted {
            continue;
        }
        let zone = derive::parse_zone(&profile.zone)?;
        let windows: Vec<_> = profile.windows.iter().map(Window::core).collect();
        let tasks = by_member.remove(&profile.member_id).unwrap_or_default();
        let today = time::to_local(now, zone).date();
        let mut new_conflicts = BTreeSet::new();
        for load in core::daily_load(&tasks, &windows, zone)
            .into_iter()
            .filter(|_| !windows.is_empty())
        {
            rows.push(LoadRow {
                member_id: profile.member_id,
                date: load.date,
                planned_minutes: load.planned_minutes as i64,
                available_minutes: load.available_minutes as i64,
                starts_at: time::start_of_day(load.date, zone),
                ends_at: time::start_of_day(
                    load.date.succ_opt().ok_or(derive::Error::NotFound)?,
                    zone,
                ),
            });
            if load.overload_minutes > 0 {
                new_conflicts.insert(load.date);
            }
        }
        if !windows.is_empty() {
            let old: BTreeSet<_> = profile
                .old_conflicts
                .iter()
                .copied()
                .filter(|d| {
                    full.contains(&profile.member_id)
                        || selected_set.contains(&(profile.member_id, *d))
                })
                .collect();
            for date in transition_dates(&old, &new_conflicts, today) {
                transitions.push(LoadTransition {
                    household_id: profile.household_id,
                    member_id: profile.member_id,
                    date,
                    conflicting: new_conflicts.contains(&date),
                });
            }
        }
    }
    sqlx::query("delete from porchlight.member_daily_load where member_id=any($1) and (member_id=any($2) or (member_id,date) in(select r.member_id,r.date from jsonb_to_recordset($3) as r(member_id uuid,date date)))")
        .bind(members).bind(&full).bind(sqlx::types::Json(&selected))
        .execute(&mut *conn)
        .await?;
    if !rows.is_empty() {
        sqlx::query("insert into porchlight.member_daily_load(member_id,date,planned_minutes,available_minutes,starts_at,ends_at,derived_at) select l.member_id,l.date,l.planned_minutes,l.available_minutes,l.starts_at,l.ends_at,$2 from jsonb_to_recordset($1) as l(member_id uuid,date date,planned_minutes bigint,available_minutes bigint,starts_at timestamptz,ends_at timestamptz) on conflict(member_id,date) do update set planned_minutes=excluded.planned_minutes,available_minutes=excluded.available_minutes,starts_at=excluded.starts_at,ends_at=excluded.ends_at,derived_at=excluded.derived_at").bind(sqlx::types::Json(&rows)).bind(now).execute(&mut *conn).await?;
    }
    // Conditional generation ack also drains jobs for deleted members.
    let acknowledgements: Vec<_> = profiles
        .iter()
        .filter(|p| full.contains(&p.member_id))
        .filter_map(|p| {
            p.seen
                .map(|seen| serde_json::json!({"member_id":p.member_id,"seen":seen}))
        })
        .collect();
    sqlx::query("update porchlight.member_load_jobs j set applied_gen=a.seen,attempts=0,last_error=null,retry_at=$2 from jsonb_to_recordset($1) as a(member_id uuid,seen bigint) where j.member_id=a.member_id and j.requested_gen=a.seen").bind(sqlx::types::Json(acknowledgements)).bind(now).execute(&mut *conn).await?;
    emit_load_notifications(conn, &transitions).await?;
    Ok(())
}
fn transition_dates(
    old: &BTreeSet<NaiveDate>,
    new: &BTreeSet<NaiveDate>,
    today: NaiveDate,
) -> Vec<NaiveDate> {
    old.symmetric_difference(new)
        .copied()
        .filter(|d| *d >= today && *d <= today + Duration::days(14))
        .collect()
}
#[cfg(test)]
mod transition_tests {
    use super::*;
    #[test]
    fn inclusive_near_term_enter_leave_only() {
        let today: NaiveDate = "2026-10-05".parse().unwrap();
        let old = BTreeSet::from([today - Duration::days(1), today, today + Duration::days(3)]);
        let new = BTreeSet::from([
            today + Duration::days(3),
            today + Duration::days(14),
            today + Duration::days(15),
        ]);
        assert_eq!(
            transition_dates(&old, &new, today),
            vec![today, today + Duration::days(14)]
        );
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct LoadTransition {
    pub household_id: Uuid,
    pub member_id: Uuid,
    pub date: NaiveDate,
    pub conflicting: bool,
}
/// One statement per recompute, including an empty R47-silent batch.
/// Member-local inclusive [today,today+14], transitions only; dedupe by member/date/state.
pub async fn emit_load_notifications(
    conn: &mut PgConnection,
    transitions: &[LoadTransition],
) -> Result<()> {
    #[cfg(feature = "testing")]
    crate::testing::record_load_hook(transitions);
    sqlx::query("select porchlight.pl_emit_notification_event(t.household_id,t.member_id,'load_conflict','member_daily_load',t.member_id,jsonb_build_object('date',t.date,'conflicting',t.conflicting),null,null,'load_conflict:'||t.member_id::text||':'||t.date::text||':'||t.conflicting::text) from jsonb_to_recordset($1) as t(household_id uuid,member_id uuid,date date,conflicting bool) join porchlight.household_members m on m.id=t.member_id and m.household_id=t.household_id where m.role<>'dependent' and m.deleted_at is null")
        .bind(sqlx::types::Json(transitions)).execute(conn).await?;
    Ok(())
}
#[derive(Serialize)]
pub struct SuggestionNotification {
    pub household: Uuid,
    pub member: Uuid,
    pub run: Uuid,
    pub item: Option<Uuid>,
    pub occurrence: Option<Uuid>,
}
/// Shared producer ABI captures actor/self suppression. EDF dedupes member/run;
/// dependent candidates dedupe member/occurrence, retaining the real run in payload.
pub async fn emit_suggestion_notifications(
    conn: &mut PgConnection,
    notifications: &[SuggestionNotification],
) -> Result<()> {
    #[cfg(feature = "testing")]
    for n in notifications {
        crate::testing::record_suggestion_hook(n.member, n.run, n.item);
    }
    if !notifications.is_empty() {
        sqlx::query("select porchlight.pl_emit_notification_event(n.household,n.member,'suggestion',case when n.item is null then 'schedule_run' else 'item' end,coalesce(n.item,n.run),jsonb_build_object('run_id',n.run),n.item,n.occurrence,case when n.item is null then 'suggestion:'||n.member::text||':'||n.run::text else 'suggestion:dependent:'||n.member::text||':'||n.occurrence::text end) from jsonb_to_recordset($1) as n(household uuid,member uuid,run uuid,item uuid,occurrence uuid) join porchlight.household_members m on m.id=n.member and m.household_id=n.household where m.role<>'dependent' and m.deleted_at is null")
            .bind(sqlx::types::Json(notifications)).execute(conn).await?;
    }
    Ok(())
}
pub async fn emit_suggestion_notification(
    conn: &mut PgConnection,
    household: Uuid,
    member: Uuid,
    run: Uuid,
    item: Option<Uuid>,
    occurrence: Option<Uuid>,
) -> Result<()> {
    emit_suggestion_notifications(
        conn,
        &[SuggestionNotification {
            household,
            member,
            run,
            item,
            occurrence,
        }],
    )
    .await
}

/// Authoritative R2 membership, using items alias i; no visibility or live/kind filter.
/// Callers provide only a trusted SQL RHS (e.g. "$2" or "any($2)"). This includes
/// subject and assignees; ownership is the fallback only when both are absent.
pub fn agenda_membership_sql(member_match: &'static str) -> String {
    format!(
        "(i.subject_member_id={member_match} or exists(select 1 from porchlight.item_assignees a where a.item_id=i.id and a.member_id={member_match}) or (i.subject_member_id is null and not exists(select 1 from porchlight.item_assignees a where a.item_id=i.id) and i.owner_id={member_match}))"
    )
}

/// No counterpart IDs/titles are disclosed by a load warning.
pub async fn load_conflicts(
    conn: &mut PgConnection,
    items: &[Uuid],
    members: Option<&[Uuid]>,
) -> Result<Vec<(Uuid, derive::Conflict)>> {
    load_conflicts_filtered(conn, items, members, None).await
}
/// Agenda page warnings are bounded to its occurrence ids, not the item horizon.
pub async fn load_conflicts_for_occurrences(
    conn: &mut PgConnection,
    items: &[Uuid],
    member: Uuid,
    occurrences: &[Uuid],
) -> Result<Vec<(Uuid, derive::Conflict)>> {
    load_conflicts_filtered(conn, items, Some(&[member]), Some(occurrences)).await
}
async fn load_conflicts_filtered(
    conn: &mut PgConnection,
    items: &[Uuid],
    members: Option<&[Uuid]>,
    occurrences: Option<&[Uuid]>,
) -> Result<Vec<(Uuid, derive::Conflict)>> {
    let rows: Vec<LoadWarningRow> = sqlx::query_as("select a.item_id,a.member_id,a.occurrence_id,l.starts_at,l.ends_at from porchlight.member_agenda a join porchlight.items i on i.id=a.item_id join porchlight.item_occurrences o on o.id=a.occurrence_id join porchlight.household_members m on m.id=a.member_id join porchlight.households h on h.id=m.household_id join porchlight.member_daily_load l on l.member_id=a.member_id and l.date=coalesce((a.starts_at at time zone case when m.role='dependent' then h.tzid else m.tzid end)::date,(a.due_at at time zone case when m.role='dependent' then h.tzid else m.tzid end)::date,a.due_date) where a.item_id=any($1) and ($2::uuid[] is null or a.member_id=any($2)) and ($3::uuid[] is null or a.occurrence_id=any($3)) and i.deleted_at is null and i.kind in ('todo','deadline') and i.est_minutes is not null and o.status in ('backlog','todo','doing') and o.disposition<>'expired' and l.planned_minutes>l.available_minutes order by a.member_id,a.occurrence_id").bind(items).bind(members).bind(occurrences).fetch_all(conn).await?;
    Ok(rows
        .into_iter()
        .map(|(item, member_id, occurrence_id, starts_at, ends_at)| {
            (
                item,
                derive::Conflict {
                    member_id,
                    occurrence_id,
                    other_occurrence_id: occurrence_id,
                    other_item_id: None,
                    other_title: "Busy".into(),
                    starts_at,
                    ends_at,
                    kind: "load".into(),
                },
            )
        })
        .collect())
}

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Suggestion {
    pub id: Uuid,
    pub household_id: Uuid,
    pub member_id: Uuid,
    pub item_id: Uuid,
    pub occurrence_id: Uuid,
    pub proposed_start: DateTime<Utc>,
    pub proposed_end: DateTime<Utc>,
    pub status: String,
    pub run_id: Uuid,
    pub version: i32,
    pub created_at: DateTime<Utc>,
}
pub async fn suggestion(conn: &mut PgConnection, household: Uuid, id: Uuid) -> Result<Suggestion> {
    sqlx::query_as("select id,household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,status,run_id,version,created_at from porchlight.schedule_suggestions where household_id=$1 and id=$2").bind(household).bind(id).fetch_optional(conn).await?.ok_or(derive::Error::NotFound)
}
/// Caller holds member locks, and has drained pending item peers through ONE WriteScope.
/// Visibility/eligibility must be checked by the API before passing allowed IDs.
pub async fn suggest(
    conn: &mut PgConnection,
    household: Uuid,
    member: Uuid,
    allowed: &[Uuid],
    now: DateTime<Utc>,
) -> Result<(Uuid, core::ScheduleResult, Vec<Suggestion>)> {
    let schedule = member_schedule(conn, household, member).await?;
    let rows: Vec<TaskRow> = sqlx::query_as("select a.occurrence_id,a.due_at,a.due_date,i.est_minutes,i.priority from porchlight.member_agenda a join porchlight.items i on i.id=a.item_id join porchlight.item_occurrences o on o.id=a.occurrence_id join porchlight.item_derivation d on d.item_id=i.id where a.member_id=$1 and a.item_id=any($2) and i.kind in ('todo','deadline') and i.deleted_at is null and i.est_minutes>0 and (a.due_at is not null or a.due_date is not null) and a.starts_at is null and o.override_local_start is null and o.status in ('backlog','todo','doing') and o.disposition<>'expired' and d.requested_gen=d.applied_gen order by a.occurrence_id").bind(member).bind(allowed).fetch_all(&mut *conn).await?;
    let tasks: Vec<_> = rows
        .into_iter()
        .map(|(id, due, date, est, priority)| core::Task {
            id,
            member_id: member,
            est_minutes: est as u32,
            deadline: due
                .map(core::Deadline::Instant)
                .or(date.map(core::Deadline::Date)),
            priority,
        })
        .collect();
    let result = core::suggest_edf(&tasks, &[schedule], now, now + Duration::days(90));
    // Retain history and excluded live items (R48). Retire every proposal of
    // considered items, including ineligible/missing occurrences, plus deleted
    // items which cannot appear in the authoritative agenda input anymore, including
    // reassignment/subject/kind changes. Excluded items still on the agenda survive.
    sqlx::query(
        sqlx::AssertSqlSafe(format!("delete from porchlight.schedule_suggestions s where member_id=$1 and status='proposed' and (item_id=any($2) or not exists(select 1 from porchlight.items i where i.id=s.item_id and i.deleted_at is null and i.kind in ('todo','deadline') and {}))", agenda_membership_sql("$1")).as_str()),
    )
    .bind(member).bind(allowed)
    .execute(&mut *conn)
    .await?;
    let run = Uuid::now_v7();
    let mut suggestions = Vec::new();
    for s in &result.suggestions {
        let id: Uuid = sqlx::query_scalar("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) select $1,$2,item_id,id,$4,$5,$6 from porchlight.item_occurrences where id=$3 returning id").bind(household).bind(member).bind(s.task_id).bind(s.range.start).bind(s.range.end).bind(run).fetch_one(&mut *conn).await?;
        suggestions.push(suggestion(conn, household, id).await?);
    }
    if !suggestions.is_empty() {
        emit_suggestion_notification(conn, household, member, run, None, None).await?;
    }
    Ok((run, result, suggestions))
}
