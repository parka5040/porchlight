//! Restart-safe notification consumer. Source links, never derived visibility, authorize inbox details.
use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc};
use chrono_tz::Tz;
use porchlight_core::time;
use porchlight_db::tx::{Actor, begin_as};
use serde::{Deserialize, Serialize};
use sqlx::{Acquire, PgConnection, PgPool, types::Json};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, utoipa::ToSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NotificationPrefs {
    pub reminders: bool,
    pub assignments: bool,
    pub dependents: bool,
    pub activity: bool,
    pub digest: bool,
    pub connection: bool,
    /// Local HH:MM:SS in the member's authoritative zone; both times null disables quiet hours.
    pub quiet_start: Option<String>,
    pub quiet_end: Option<String>,
    pub default_todo_offsets: Vec<i32>,
    pub default_deadline_offsets: Vec<i32>,
    pub default_appointment_offsets: Vec<i32>,
}
impl NotificationPrefs {
    pub fn enabled(&self, category: &str) -> bool {
        match category {
            "reminders" => self.reminders,
            "assignments" => self.assignments,
            "dependents" => self.dependents,
            "digest" => self.digest,
            "connection" => self.connection,
            _ => self.activity,
        }
    }
}
pub const PREFS_JSON: &str = "jsonb_build_object('reminders',p.reminders,'assignments',p.assignments,'dependents',p.dependents,'activity',p.activity,'digest',p.digest,'connection',p.connection,'quiet_start',p.quiet_start,'quiet_end',p.quiet_end,'default_todo_offsets',p.default_todo_offsets,'default_deadline_offsets',p.default_deadline_offsets,'default_appointment_offsets',p.default_appointment_offsets)";

pub fn category(kind: &str) -> &'static str {
    match kind {
        "reminder" => "reminders",
        "assigned" => "assignments",
        "dependent_unassigned" => "dependents",
        _ => "activity",
    }
}
/// Half-open local quiet interval. Fold/gap resolution shares the core's earlier/next-valid rule.
pub fn quiet_release(
    now: DateTime<Utc>,
    zone: Tz,
    start: Option<NaiveTime>,
    end: Option<NaiveTime>,
) -> Option<DateTime<Utc>> {
    let (start, end) = (start?, end?);
    if start == end {
        return None;
    }
    let date = time::to_local(now, zone).date();
    // Compare instants, not wall-clock times: during a fold the second 01:15
    // can lie AFTER an earlier-fold 01:30 start. Check yesterday as well for
    // overnight intervals. Resolve both endpoints with the shared core policy.
    [Some(date), date.pred_opt()]
        .into_iter()
        .flatten()
        .find_map(|date| {
            let end_date = if start > end { date.succ_opt()? } else { date };
            let start = time::resolve_local(date.and_time(start), zone);
            let end = time::resolve_local(end_date.and_time(end), zone);
            (start <= now && now < end).then_some(end)
        })
}
/// Weekly slot is Sunday 18:00 before the appointment's Monday–Sunday week.
/// Countdown slots are exact elapsed hours, not DST-sensitive wall-day subtraction.
pub fn dependent_slots(start: DateTime<Utc>, zone: Tz) -> [(&'static str, DateTime<Utc>); 3] {
    let date = time::to_local(start, zone).date();
    let sunday = date - Duration::days(i64::from(date.weekday().num_days_from_monday()) + 1);
    [
        (
            "weekly",
            time::resolve_local(sunday.and_hms_opt(18, 0, 0).unwrap(), zone),
        ),
        ("48h", start - Duration::hours(48)),
        ("24h", start - Duration::hours(24)),
    ]
}
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Source {
    pub id: Uuid,
    pub kind: String,
    pub item_id: Option<Uuid>,
    pub occurrence_id: Option<Uuid>,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub actor_member_id: Option<Uuid>,
    pub actor_label: Option<String>,
    pub actor_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub conflicting: Option<bool>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct NotificationSummary {
    /// Short legacy/CLI rendering; never includes producer payload strings.
    pub text: String,
    pub title: String,
    pub body: String,
    /// Presentation topic. Delivery channels remain the spec's six preference
    /// categories: comments/conflicts/planning/operational alerts use activity.
    pub category: String,
    pub local_alarm: bool,
}
/// Titles/bodies are static safe templates, not producer payloads (which may
/// contain URLs, credentials, comment text or private planning data).
fn template(kind: &str, conflicting: Option<bool>) -> (&'static str, &'static str, &'static str) {
    match kind {
        "assigned" => (
            "New assignment",
            "An item was assigned to you.",
            "assignments",
        ),
        "status_changed" => (
            "Status changed",
            "An item's status has changed.",
            "activity",
        ),
        "completed" => ("Item completed", "An item has been completed.", "activity"),
        "comment" => ("New comment", "A comment was added to an item.", "comments"),
        "mention" => (
            "You were mentioned",
            "You were mentioned in an item comment.",
            "comments",
        ),
        "conflict" => (
            "Calendar conflict",
            "An appointment overlaps another calendar entry.",
            "conflicts",
        ),
        "load_conflict" if conflicting == Some(false) => (
            "Availability restored",
            "Load back within availability.",
            "conflicts",
        ),
        "load_conflict" => (
            "Availability exceeded",
            "Planned load exceeds availability.",
            "conflicts",
        ),
        "reminder" => ("Reminder", "An item is due for a reminder.", "reminders"),
        "dependent_unassigned" => (
            "Unassigned appointment",
            "A dependent appointment needs an assignee.",
            "dependents",
        ),
        "derivation_error" => (
            "Processing needs attention",
            "A scheduled item could not be processed.",
            "operational",
        ),
        "backup_stale" => (
            "Backup needs attention",
            "No successful backup for more than 26 hours.",
            "operational",
        ),
        "feed_error" => (
            "Calendar feed needs attention",
            "A calendar feed could not be refreshed.",
            "operational",
        ),
        "calendar_changed" => (
            "Calendar updated",
            "A calendar event has changed.",
            "activity",
        ),
        "suggestion" => (
            "Planning suggestion",
            "A planning suggestion is available.",
            "activity",
        ),
        _ => (
            "Porchlight activity",
            "New household activity is available.",
            "activity",
        ),
    }
}
pub fn summary(sources: &[Source], digest: bool) -> NotificationSummary {
    let Some(first) = sources.first() else {
        return NotificationSummary {
            text: "Porchlight activity".into(),
            title: "Porchlight activity".into(),
            body: "New household activity is available.".into(),
            category: "activity".into(),
            local_alarm: false,
        };
    };
    let actor = if sources
        .iter()
        .all(|s| s.actor_member_id == first.actor_member_id && s.actor_label == first.actor_label)
    {
        first
            .actor_label
            .as_deref()
            .or(first.actor_name.as_deref())
            .unwrap_or("Porchlight")
    } else {
        "Household activity"
    };
    let (title, body, topic) = template(&first.kind, first.conflicting);
    let n = sources.len();
    let homogeneous = sources.iter().all(|s| s.kind == first.kind);
    let (title, body, topic, text) = if digest {
        let text = format!(
            "Morning digest: {n} {}",
            if n == 1 { "update" } else { "updates" }
        );
        ("Morning digest", text.clone(), "digest", text)
    } else if n > 1 {
        let text = match first.kind.as_str() {
            "reminder" if homogeneous => format!("Reminder history: {n} reminders"),
            "dependent_unassigned" if homogeneous => format!("Dependent appointments: {n} alerts"),
            _ => format!("{actor}: {n} household updates"),
        };
        // Neutral wording also for mixed active/cleared load-conflict sources.
        (
            "Household updates",
            text.clone(),
            if homogeneous { topic } else { "activity" },
            text,
        )
    } else {
        (title, body.to_string(), topic, format!("{actor}: {title}"))
    };
    NotificationSummary {
        text,
        title: title.into(),
        body,
        category: topic.into(),
        local_alarm: sources.iter().all(|s| s.kind == "reminder"),
    }
}
/// Sources are reauthorized independently, preventing both title and coalesced-count leaks.
pub async fn visible_sources(
    c: &mut PgConnection,
    id: Uuid,
    member: Uuid,
    household: Uuid,
) -> sqlx::Result<Vec<Source>> {
    sqlx::query_as("select e.id,e.kind,e.item_id,e.occurrence_id,e.entity_type,e.entity_id,e.actor_member_id,e.actor_label,m.display_name as actor_name,e.created_at,case when jsonb_typeof(e.payload->'conflicting')='boolean' then (e.payload->>'conflicting')::boolean end as conflicting from porchlight.notification_sources s join porchlight.notification_events e on e.id=s.event_id left join porchlight.household_members m on m.id=e.actor_member_id where s.notification_id=$1 and e.household_id=$3 and e.recipient_member_id=$2 and (e.item_id is null or porchlight.pl_item_can_read(e.item_id,$2)) order by e.created_at,e.id")
        .bind(id).bind(member).bind(household).fetch_all(c).await
}
/// Member FK lock precedes prefs in every consumer/release path. Never wait on a
/// member while holding prefs; skip busy recipients and try them next tick.
async fn lock_member(c: &mut PgConnection, member: Uuid) -> sqlx::Result<bool> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "select id from porchlight.household_members where id=$1 for key share skip locked",
    )
    .bind(member)
    .fetch_optional(c)
    .await?
    .is_some())
}
fn error_class(error: &anyhow::Error) -> &str {
    if let Some(sqlx::Error::Database(db)) = error.downcast_ref::<sqlx::Error>() {
        match db.code().as_deref() {
            Some("40P01") => "deadlock",
            Some("40001") => "serialization",
            Some("55P03") => "lock_timeout",
            Some("57014") => "statement_cancel",
            Some(code) if code.starts_with("23") => "constraint",
            _ => "database",
        }
    } else if error.downcast_ref::<sqlx::Error>().is_some() {
        "database_client"
    } else {
        "invalid_data"
    }
}
/// Recipient serialization with atomic per-event savepoints. Durable bounded
/// backoff survives restarts; isolated transient failures do not stall release.
pub async fn drain(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<usize> {
    let recipients:Vec<(Uuid,Uuid)>=sqlx::query_as("select e.recipient_member_id,e.household_id from porchlight.notification_events e left join porchlight.notification_delivery_retry r on r.member_id=e.recipient_member_id where e.processed_at is null and e.created_at<=$1 and (r.retry_at is null or r.retry_at<=$1) and not exists(select 1 from porchlight.notification_event_delivery d where d.event_id=e.id and d.retry_at>$1) and not exists(select 1 from porchlight.notification_events pending join porchlight.notification_event_delivery d on d.event_id=pending.id where pending.recipient_member_id=e.recipient_member_id and pending.processed_at is null and d.retry_at>$1 and d.last_error not in ('deadlock','serialization','lock_timeout','statement_cancel')) group by e.recipient_member_id,e.household_id order by min(e.created_at),e.recipient_member_id limit 50").bind(now).fetch_all(pool).await?;
    let mut processed = 0;
    for (member, household) in recipients {
        match consume_recipient(pool, now, member, household).await {
            Ok(count) => processed += count,
            Err(error) => {
                tracing::warn!(%member, class=error_class(&error), "notification recipient failed; backing off");
                if let Err(error) = backoff(pool, now, member).await {
                    tracing::warn!(%member, class=error_class(&error), "notification backoff failed");
                }
            }
        }
    }
    let pending:Vec<(Uuid,Uuid)>=sqlx::query_as("select distinct n.recipient_member_id,n.household_id from porchlight.notifications n left join porchlight.notification_delivery_retry r on r.member_id=n.recipient_member_id where n.released_at is null and n.release_at<=$1 and (r.retry_at is null or r.retry_at<=$1 or exists(select 1 from porchlight.notification_events e join porchlight.notification_event_delivery d on d.event_id=e.id where e.recipient_member_id=n.recipient_member_id and e.processed_at is null and d.retry_at>$1 and d.last_error in ('deadlock','serialization','lock_timeout','statement_cancel'))) order by n.recipient_member_id limit 50").bind(now).fetch_all(pool).await?;
    for (member, household) in pending {
        if let Err(error) = release_recipient(pool, now, member, household).await {
            tracing::warn!(%member, class=error_class(&error), "notification release failed; retrying");
            if let Err(error) = backoff(pool, now, member).await {
                tracing::warn!(%member,class=error_class(&error),"notification release backoff failed");
            }
        }
    }
    Ok(processed)
}
async fn backoff(pool: &PgPool, now: DateTime<Utc>, member: Uuid) -> anyhow::Result<()> {
    let mut tx = begin_as(pool, &Actor::system()).await?;
    if lock_member(&mut tx, member).await? {
        backoff_in(&mut tx, now, member).await?;
    }
    tx.commit().await?;
    Ok(())
}
// Called under member/prefs serialization; never opens a second lock scope.
async fn backoff_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    now: DateTime<Utc>,
    member: Uuid,
) -> anyhow::Result<()> {
    sqlx::query("insert into porchlight.notification_delivery_retry(member_id,attempts,retry_at) values($1,1,$2+interval '2 seconds') on conflict(member_id) do update set attempts=least(notification_delivery_retry.attempts+1,6),retry_at=$2+make_interval(secs=>least(60,power(2,notification_delivery_retry.attempts+1))::double precision)").bind(member).bind(now).execute(&mut **tx).await?;
    Ok(())
}

// R83: generous outage grace, bounded by immutable event age (injected clock).
const TRANSIENT_GRACE: chrono::Duration = chrono::Duration::hours(6);

async fn release_recipient(
    pool: &PgPool,
    now: DateTime<Utc>,
    member: Uuid,
    household: Uuid,
) -> anyhow::Result<()> {
    let mut tx = begin_as(pool, &Actor::system()).await?;
    if lock_member(&mut tx, member).await? {
        let locked:Option<bool>=sqlx::query_scalar("select digest from porchlight.notification_prefs where member_id=$1 and household_id=$2 for update skip locked").bind(member).bind(household).fetch_optional(&mut *tx).await?;
        if let Some(digest) = locked {
            // Release and suppression also share purge/read-all's UUID order.
            sqlx::query("select id from porchlight.notifications where recipient_member_id=$1 and released_at is null order by id for update")
                .bind(member).fetch_all(&mut *tx).await?;
            if !digest {
                // Suppress undelivered alerts, not already-released history.
                // Event rows remain immutable history; deleting these held rows
                // cascades only their source links and emits no outbox tickle.
                sqlx::query("delete from porchlight.notifications where recipient_member_id=$1 and category='digest' and released_at is null")
                    .bind(member).execute(&mut *tx).await?;
            }
            let released = sqlx::query("update porchlight.notifications set released_at=$2 where recipient_member_id=$1 and released_at is null and release_at<=$2 and not exists(select 1 from porchlight.notification_events e where e.recipient_member_id=$1 and e.processed_at is null and e.created_at<=$2 and not exists(select 1 from porchlight.notification_event_delivery d where d.event_id=e.id and d.retry_at>$2 and d.last_error in ('deadlock','serialization','lock_timeout','statement_cancel')))").bind(member).bind(now).execute(&mut *tx).await?;
            if released.rows_affected() > 0 {
                sqlx::query(
                    "delete from porchlight.notification_delivery_retry r where member_id=$1 and not exists(select 1 from porchlight.notification_events e join porchlight.notification_event_delivery d on d.event_id=e.id where e.recipient_member_id=r.member_id and e.processed_at is null)",
                )
                .bind(member)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}
async fn consume_recipient(
    pool: &PgPool,
    now: DateTime<Utc>,
    member: Uuid,
    household: Uuid,
) -> anyhow::Result<usize> {
    let mut tx = begin_as(pool, &Actor::system()).await?;
    if !lock_member(&mut tx, member).await? {
        return Ok(0);
    }
    let mut processed = 0;
    let p:Option<(Json<NotificationPrefs>,String,bool)>=sqlx::query_as(sqlx::AssertSqlSafe(format!("select {},case when m.role='dependent' then h.tzid else m.tzid end,m.deleted_at is null and m.role<>'dependent' from porchlight.notification_prefs p join porchlight.household_members m on m.id=p.member_id join porchlight.households h on h.id=p.household_id where p.member_id=$1 and p.household_id=$2 for update of p skip locked", PREFS_JSON)))
            .bind(member).bind(household).fetch_optional(&mut *tx).await?;
    let Some((Json(p), zone, live)) = p else {
        tx.rollback().await?;
        return Ok(0);
    };
    // Candidate discovery is unlocked: a competing consumer may have committed
    // a backoff before we acquired prefs. Recheck under the serialization lock.
    let backed_off: bool = sqlx::query_scalar("select exists(select 1 from porchlight.notification_delivery_retry where member_id=$1 and retry_at>$2) or exists(select 1 from porchlight.notification_events e join porchlight.notification_event_delivery d on d.event_id=e.id where e.recipient_member_id=$1 and e.processed_at is null and d.retry_at>$2 and d.last_error not in ('deadlock','serialization','lock_timeout','statement_cancel'))")
        .bind(member).bind(now).fetch_one(&mut *tx).await?;
    if backed_off {
        tx.rollback().await?;
        return Ok(0);
    }
    let zone = time::parse_tz(&zone).ok_or_else(|| anyhow::anyhow!("invalid stored zone"))?;
    let events:Vec<Source>=sqlx::query_as("select e.id,e.kind,e.item_id,e.occurrence_id,e.entity_type,e.entity_id,e.actor_member_id,e.actor_label,m.display_name as actor_name,e.created_at,case when jsonb_typeof(e.payload->'conflicting')='boolean' then (e.payload->>'conflicting')::boolean end as conflicting from porchlight.notification_events e left join porchlight.household_members m on m.id=e.actor_member_id where e.recipient_member_id=$1 and e.household_id=$2 and e.processed_at is null and e.created_at<=$3 and not exists(select 1 from porchlight.notification_event_delivery d where d.event_id=e.id and d.retry_at>$3) order by e.created_at,e.id limit 500")
            .bind(member).bind(household).bind(now).fetch_all(&mut *tx).await?;
    // Physical purge takes item -> event(id) -> notification locks. Reserve
    // all referenced items without waiting (notification INSERT has an item FK),
    // then all batch events in UUID order BEFORE touching any notification.
    // Per-event savepoints retain earlier successes' locks until recipient commit;
    // acquiring event locks inside that loop would invert purge's order.
    let mut items: Vec<_> = events.iter().filter_map(|e| e.item_id).collect();
    items.sort_unstable();
    items.dedup();
    let locked_items: Vec<Uuid> = sqlx::query_scalar(
        "select id from porchlight.items where id=any($1) order by id for key share skip locked",
    )
    .bind(&items)
    .fetch_all(&mut *tx)
    .await?;
    if locked_items.len() != items.len() {
        tx.rollback().await?;
        return Ok(0);
    }
    let ids: Vec<_> = events.iter().map(|e| e.id).collect();
    let locked_events: Vec<Uuid> = sqlx::query_scalar("select id from porchlight.notification_events where id=any($1) and processed_at is null order by id for update")
        .bind(&ids).fetch_all(&mut *tx).await?;
    let locked_events: std::collections::BTreeSet<_> = locked_events.into_iter().collect();
    // Purge/retention lock notifications by UUID, not chronological processing
    // order. Reserve ALL possible existing coalescing targets after event locks.
    // Prefs exclude other consumers inserting a target; read-all only narrows
    // the unread set. Rows inserted by this transaction need no reservation.
    if let (Some(first), Some(last)) = (events.first(), events.last()) {
        let releases: Vec<_> = events
            .iter()
            .filter_map(|e| {
                quiet_release(
                    e.created_at,
                    zone,
                    p.quiet_start.as_deref().and_then(|t| t.parse().ok()),
                    p.quiet_end.as_deref().and_then(|t| t.parse().ok()),
                )
            })
            .collect();
        sqlx::query("select id from porchlight.notifications where recipient_member_id=$1 and ((category='digest' and release_at=any($2)) or (category<>'digest' and read_at is null and window_start<=$4 and window_start+interval '30 seconds'>$3)) order by id for update")
            .bind(member).bind(&releases).bind(first.created_at).bind(last.created_at)
            .fetch_all(&mut *tx).await?;
    }
    let mut transient_failed = false;
    // N1: 55P03/57014 cost seconds each; allow one per recipient pass.
    let mut timeout_stopped = false;
    for event in events.into_iter().filter(|e| locked_events.contains(&e.id)) {
        // The recipient prefs lock serializes consumers. An event savepoint keeps
        // notification/source/outbox/processed_at atomic without losing earlier
        // successes or the durable failure record on one poisoned event.
        let mut event_tx = tx.begin().await?;
        let result: anyhow::Result<()> = async {
        let release = quiet_release(
            event.created_at,
            zone,
            p.quiet_start.as_deref().and_then(|t| t.parse().ok()),
            p.quiet_end.as_deref().and_then(|t| t.parse().ok()),
        );
        let cat = category(&event.kind);
        let visible: bool =
            sqlx::query_scalar("select $1::uuid is null or porchlight.pl_item_can_read($1,$2)")
                .bind(event.item_id)
                .bind(member)
                .fetch_one(&mut *event_tx)
                .await?;
        let held = cat != "reminders" && release.is_some();
        if live && visible && p.enabled(cat) && (!held || p.digest) {
            let target:Option<Uuid>=sqlx::query_scalar("select id from porchlight.notifications where recipient_member_id=$1 and (($2 and category='digest' and release_at=$3) or (not $2 and read_at is null and category<>'digest' and (category='reminders')=$4 and window_start<=$5 and $5<window_start+interval '30 seconds')) order by window_start desc,id desc limit 1 for update")
                    .bind(member).bind(held).bind(release.unwrap_or(now)).bind(cat=="reminders").bind(event.created_at).fetch_optional(&mut *event_tx).await?;
            let id = if let Some(id) = target {
                id
            } else {
                sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,item_id,occurrence_id,entity_type,entity_id,actor_member_id,actor_label,window_start,release_at,released_at,summary) values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) returning id")
                        .bind(household).bind(member).bind(&event.kind).bind(if held {"digest"} else {cat}).bind(event.item_id).bind(event.occurrence_id).bind(&event.entity_type).bind(event.entity_id).bind(event.actor_member_id).bind(&event.actor_label).bind(event.created_at).bind(release.filter(|_|held).unwrap_or(now)).bind(if held {None} else {Some(now)}).bind(Json(serde_json::to_value(summary(std::slice::from_ref(&event),held))?)).fetch_one(&mut *event_tx).await?
            };
            sqlx::query("insert into porchlight.notification_sources(notification_id,event_id) values($1,$2)").bind(id).bind(event.id).execute(&mut *event_tx).await?;
            if target.is_some() {
                let sources = visible_sources(&mut event_tx, id, member, household).await?;
                sqlx::query("update porchlight.notifications set count=count+1,kind='summary',category=case when category=$3 then category when category='digest' then 'digest' else 'activity' end,summary=$2,item_id=null,occurrence_id=null,actor_member_id=null,actor_label=null where id=$1")
                        .bind(id).bind(Json(serde_json::to_value(summary(&sources,held))?)).bind(cat).execute(&mut *event_tx).await?;
            }
        }
        sqlx::query("update porchlight.notification_events set processed_at=$2 where id=$1")
            .bind(event.id)
            .bind(now)
            .execute(&mut *event_tx)
            .await?;
            Ok(())
        }.await;
        match result {
            Ok(()) => {
                event_tx.commit().await?;
                sqlx::query("delete from porchlight.notification_event_delivery where event_id=$1")
                    .bind(event.id)
                    .execute(&mut *tx)
                    .await?;
                processed += 1;
            }
            Err(error) => {
                event_tx.rollback().await?;
                let transient = error
                    .downcast_ref::<sqlx::Error>()
                    .and_then(|e| e.as_database_error())
                    .is_some_and(|e| {
                        matches!(
                            e.code().as_deref(),
                            Some("40P01" | "40001" | "55P03" | "57014")
                        )
                    });
                let class = error_class(&error);
                let timeout = transient && matches!(class, "lock_timeout" | "statement_cancel");
                if transient && now - event.created_at < TRANSIENT_GRACE {
                    // No strike within grace; retain any prior data strikes. The
                    // next event still runs, and previous deliveries remain atomic.
                    sqlx::query("insert into porchlight.notification_event_delivery(event_id,attempts,retry_at,last_error) values($1,0,$2+make_interval(secs=>least(60,power(2,coalesce((select attempts from porchlight.notification_delivery_retry where member_id=$3),0)+1))::double precision),$4) on conflict(event_id) do update set retry_at=excluded.retry_at,last_error=excluded.last_error")
                        .bind(event.id).bind(now).bind(member).bind(class).execute(&mut *tx).await?;
                    transient_failed = true;
                    tracing::warn!(%member,event=%event.id,class,"notification event transient; backing off within grace");
                    if timeout {
                        timeout_stopped = true;
                        break;
                    }
                    continue;
                }
                let attempts: i32 = sqlx::query_scalar("insert into porchlight.notification_event_delivery(event_id,attempts,retry_at,last_error) values($1,1,$2+interval '2 seconds',$3) on conflict(event_id) do update set attempts=least(notification_event_delivery.attempts+1,5),retry_at=$2+make_interval(secs=>power(2,least(notification_event_delivery.attempts+1,5))::double precision),last_error=$3,poisoned_at=case when notification_event_delivery.attempts>=4 then $2 end returning attempts")
                    .bind(event.id).bind(now).bind(class).fetch_one(&mut *tx).await?;
                if transient && attempts < 5 {
                    sqlx::query("update porchlight.notification_event_delivery set retry_at=$2+make_interval(secs=>least(60,power(2,coalesce((select attempts from porchlight.notification_delivery_retry where member_id=$3),0)+1))::double precision) where event_id=$1")
                        .bind(event.id).bind(now).bind(member).execute(&mut *tx).await?;
                    transient_failed = true;
                    timeout_stopped |= timeout;
                }
                if attempts == 5 {
                    sqlx::query(
                        "update porchlight.notification_events set processed_at=$2 where id=$1",
                    )
                    .bind(event.id)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
                    tracing::warn!(%member,event=%event.id,class,attempts,"notification event poisoned; skipped");
                    processed += 1;
                    // Terminal events no longer block later events or digest release.
                    if timeout {
                        // N1: the poisoning attempt also burned a timeout; stop the pass.
                        timeout_stopped = true;
                        break;
                    }
                } else {
                    tracing::warn!(%member,event=%event.id,class,attempts,"notification event failed; backing off");
                    if !transient || timeout {
                        break;
                    }
                }
            }
        }
    }
    if transient_failed {
        backoff_in(&mut tx, now, member).await?;
        if timeout_stopped {
            // Keep attempts growing the per-event backoff, but do not park the recipient:
            // the failed event's own retry_at excludes it so later events run next pass.
            sqlx::query(
                "update porchlight.notification_delivery_retry set retry_at=$2 where member_id=$1",
            )
            .bind(member)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    } else {
        sqlx::query("delete from porchlight.notification_delivery_retry where member_id=$1")
            .bind(member)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(processed)
}
pub async fn run(
    pool: PgPool,
    clock: std::sync::Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    shutdown: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()> {
    tokio::pin!(shutdown);
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {biased; ()=&mut shutdown=>return Ok(()),_=interval.tick()=>{}}
        // Finish an in-flight atomic batch on shutdown; never detach work from lifecycle.
        if let Err(error) = drain(&pool, clock()).await {
            tracing::warn!(
                class = error_class(&error),
                "notification batch failed; retrying"
            );
        }
    }
}
/// Scheduler callbacks only touch events, never lock items after the elected scheduler row.
pub fn jobs() -> Vec<crate::scheduler::Job> {
    use crate::scheduler::{Job, Schedule};
    vec![
        Job::new(
            "notification_reminders",
            Schedule::Every(std::time::Duration::from_secs(60)),
            |c, now| Box::pin(reminders(c, now)),
        ),
        Job::new(
            "notification_dependents",
            Schedule::Every(std::time::Duration::from_secs(60)),
            |c, now| Box::pin(dependents(c, now)),
        ),
    ]
}
pub async fn reminders(c: &mut PgConnection, now: DateTime<Utc>) -> anyhow::Result<()> {
    sqlx::query("select set_config('porchlight.notification_now',$1,true)")
        .bind(now.to_rfc3339())
        .execute(&mut *c)
        .await?;
    sqlx::query("select porchlight.pl_emit_notification_event(r.household_id,r.member_id,'reminder','occurrence',r.occurrence_id,jsonb_build_object('local_alarm',true),r.item_id,r.occurrence_id,'reminder:'||r.occurrence_id::text||':'||r.member_id::text||':'||r.reminder_id::text) from porchlight.scheduled_reminders r join porchlight.item_occurrences o on o.id=r.occurrence_id join porchlight.items i on i.id=r.item_id join porchlight.item_derivation d on d.item_id=i.id and d.requested_gen=d.applied_gen join porchlight.household_members m on m.id=r.member_id where r.fire_at<=$1 and r.fire_at>$1-make_interval(days=>$2) and o.status not in ('done','skipped') and o.disposition<>'expired' and i.deleted_at is null and m.deleted_at is null and m.role<>'dependent' and porchlight.pl_item_can_read(i.id,m.id) and not exists(select 1 from porchlight.notification_events e where e.household_id=r.household_id and e.dedupe_key='reminder:'||r.occurrence_id::text||':'||r.member_id::text||':'||r.reminder_id::text)")
        .bind(now).bind(porchlight_db::derive::NOTIFICATION_EVENT_RETENTION_DAYS).execute(c).await?;
    Ok(())
}
#[derive(sqlx::FromRow)]
struct DependentRow {
    item: Json<porchlight_db::derive::ItemRow>,
    occurrence: Json<porchlight_db::derive::OccurrenceRow>,
    zone: String,
}
pub async fn dependents(c: &mut PgConnection, now: DateTime<Utc>) -> anyhow::Result<()> {
    sqlx::query("select set_config('porchlight.notification_now',$1,true)")
        .bind(now.to_rfc3339())
        .execute(&mut *c)
        .await?;
    let rows:Vec<DependentRow>=sqlx::query_as("select to_jsonb(i) || jsonb_build_object('completion_interval',porchlight.pl_interval_json(i.completion_interval)) as item,to_jsonb(o) as occurrence,h.tzid as zone from porchlight.items i join porchlight.item_occurrences o on o.item_id=i.id join porchlight.item_derivation d on d.item_id=i.id and d.requested_gen=d.applied_gen join porchlight.household_members s on s.id=i.subject_member_id join porchlight.households h on h.id=i.household_id where i.kind='appointment' and i.deleted_at is null and s.role='dependent' and s.deleted_at is null and o.status not in ('done','skipped') and o.disposition<>'expired' and not exists(select 1 from porchlight.item_assignees a where a.item_id=i.id)").fetch_all(&mut *c).await?;
    for row in rows {
        let h = row.item.household_id;
        let item = row.item.id;
        let occ = row.occurrence.id;
        let zone =
            time::parse_tz(&row.zone).ok_or_else(|| anyhow::anyhow!("invalid stored zone"))?;
        let Ok(anchor) = row
            .item
            .spec()
            .and_then(|spec| porchlight_db::derive::effective_anchor(&row.occurrence, &spec))
        else {
            tracing::warn!(%item,"dependent notification anchor invalid; left for derivation worker");
            continue;
        };
        let start = anchor
            .starts_at
            .or_else(|| anchor.local_start.map(|l| time::resolve_local(l, zone)))
            .or_else(|| anchor.due_date.map(|d| time::start_of_day(d, zone)));
        let Some(start) = start.filter(|s| *s > now) else {
            continue;
        };
        let members:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.household_members where household_id=$1 and role in ('user','super_admin') and deleted_at is null order by id").bind(h).fetch_all(&mut *c).await?;
        for (slot, due) in dependent_slots(start, zone) {
            if due > now {
                continue;
            }
            for member in &members {
                sqlx::query("select porchlight.pl_emit_notification_event($1,$2,'dependent_unassigned','occurrence',$3,jsonb_build_object('slot',$4::text),$5,$3,$6)")
                    .bind(h).bind(member).bind(occ).bind(slot).bind(item).bind(format!("dependent:{occ}:{member}:{slot}:{}",due.to_rfc3339())).execute(&mut *c).await?;
            }
        }
    }
    Ok(())
}
