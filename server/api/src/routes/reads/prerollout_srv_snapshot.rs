//! The pre-optimization implementation is retained only in this test module.
#![allow(unexpected_cfgs)] // Set only by the isolated allocation probe command.
use super::*;
#[cfg(porchlight_snapshot_probe)]
#[path = "snapshot_alloc_probe.rs"]
mod alloc_probe;
#[path = "snapshot_fixture.rs"]
mod fixture;

async fn legacy_snapshot(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    let permit = state.admit_snapshot(&caller).await?;
    let now = (state.item_clock)();
    let mut tx = porchlight_db::tx::begin(&state.pool).await?;
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *tx)
        .await?;
    let cursor: i64 = sqlx::query_scalar(
        "select coalesce(max(stream_seq),0)
        from porchlight.outbox",
    )
    .fetch_one(&mut *tx)
    .await?;
    let items = rows(
        &mut tx,
        &caller,
        &format!(
            "select {ITEM}
        {JOIN}
        where {VISIBLE}
        order by i.id"
        ),
    )
    .await?;
    // Membership uses the effective anchors, not the recurrence identity's generated
    // date. Resolve in Rust with derivation's earlier-fold/next-valid-gap semantics;
    // PostgreSQL AT TIME ZONE chooses different fold/gap instants.
    let household_zone: String =
        sqlx::query_scalar("select tzid from porchlight.households where id=$1")
            .bind(caller.household_id())
            .fetch_one(&mut *tx)
            .await?;
    let household_zone = porchlight_db::derive::parse_zone(&household_zone)
        .map_err(|_| ApiError::internal("snapshot_zone"))?;
    let timing: std::collections::HashMap<Uuid, porchlight_db::derive::ItemRow> = items
        .iter()
        .filter_map(|v| match porchlight_db::derive::ItemRow::deserialize(v) {
            Ok(row) => Some((row.id, row)),
            Err(_) => {
                // Raw-SQL poison must not take down the whole household cache.
                // Keep JSON unchanged; log only identity/category, never input values.
                tracing::warn!(item_id=?v.get("id"), "snapshot item timing unavailable");
                None
            }
        })
        .collect();
    let from = now - Duration::days(180);
    let to = now
        .checked_add_months(Months::new(18))
        .ok_or_else(|| ApiError::internal("horizon"))?;
    let candidates = legacy_candidates(
        &mut tx,
        caller.household_id(),
        caller.member_id(),
        caller.sees_all(),
        from,
        to,
    )
    .await?;
    let mut occurrences = Vec::new();
    for value in candidates {
        let exact = (|| -> std::result::Result<bool, &'static str> {
            let row = porchlight_db::derive::OccurrenceRow::deserialize(&value.0)
                .map_err(|_| "occurrence")?;
            let item = timing.get(&row.item_id).ok_or("item")?;
            let fixed_zone = item
                .timing
                .tzid
                .as_deref()
                .map(porchlight_db::derive::parse_zone)
                .transpose()
                .map_err(|_| "zone")?;
            let anchor = porchlight_db::derive::effective_anchor_with_zone(
                &row,
                item.timing.time_mode,
                fixed_zone,
            )
            .map_err(|_| "override")?;
            Ok(snapshot_in_window(&anchor, household_zone, from, to))
        })();
        let include = match exact {
            Ok(include) => include,
            Err(category) => {
                tracing::warn!(occurrence_id=?value.0.get("id"), category, "snapshot timing refinement unavailable");
                true // SQL-bounded fail-open for this row; retain authoritative JSON.
            }
        };
        if include {
            occurrences.push(value);
        }
    }
    let occurrence_ids: Vec<Uuid> = occurrences.iter().filter_map(|v| uid(&v.0["id"])).collect();
    let reminders_sql = format!(
        "select to_jsonb(r)
        from porchlight.scheduled_reminders r
        join porchlight.items i on i.id=r.item_id
        where {VISIBLE}
        and ($3 or r.member_id=$2) and r.fire_at >= $4 and r.fire_at < $5
        and r.occurrence_id=any($6)
        order by r.fire_at,r.id"
    );
    let scheduled_reminders: Vec<SqlJson<Value>> =
        sqlx::query_scalar(sqlx::AssertSqlSafe(reminders_sql.as_str()))
            .bind(caller.household_id())
            .bind(caller.member_id())
            .bind(caller.sees_all())
            .bind(now - Duration::days(180))
            .bind(
                now.checked_add_months(Months::new(18))
                    .ok_or_else(|| ApiError::internal("horizon"))?,
            )
            .bind(&occurrence_ids)
            .fetch_all(&mut *tx)
            .await?;
    let members =
        crate::routes::members::all(&mut tx, caller.household_id(), caller.member_id()).await?;
    let settings = crate::routes::members::settings_row(&mut tx, &caller).await?;
    let (notifications, notification_prefs) =
        crate::routes::notifications::snapshot_fields(&mut tx, &caller).await?;
    let ids: Vec<Uuid> = items.iter().filter_map(|v| uid(&v["id"])).collect();
    let comments = crate::collaboration::snapshot_comments_s4b(&mut tx, &caller, &ids).await?;
    let attachments = crate::blobs::snapshot_attachments_s4b(&mut tx, &caller, &ids).await?;
    let ics_sources = crate::calendar::snapshot_sources(&mut tx, &caller).await?;
    let planning = crate::planning::snapshot_contribution(&mut tx, &caller).await?;
    let lists = crate::lists::snapshot_contribution(&mut tx, &caller).await?;
    let response = Json(
        json!({"comments":comments,"attachments":attachments,"ics_sources":ics_sources,"lists":lists,"notifications":notifications,"notification_prefs":notification_prefs,"availability":planning.availability,"suggestions":planning.suggestions,"members":members,"items":items,"occurrences":occurrences.into_iter().map(|v|v.0).collect::<Vec<_>>(),"member_settings":settings,"scheduled_reminders":scheduled_reminders.into_iter().map(|v|v.0).collect::<Vec<_>>(),"cursor":cursor}),
    ).into_response();
    tx.commit().await?;
    Ok(crate::concurrency::with_body(response, permit))
}
async fn legacy_candidates(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    household: Uuid,
    member: Uuid,
    sees_all: bool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<SqlJson<Value>>, ApiError> {
    // SQL SUPERSET, not membership: separate generated and effective wall boxes
    // avoid bridging the window between an old identity and a far-future override.
    // Override shifts are unbounded, so include them directly. Local endpoints
    // differ from resolved UTC by <3 days: resolve_local walks <48h and offsets
    // are <24h. R10's corrected end has the same shift as its resolved start.
    // Date-only rows include the following midnight; undated rows always survive.
    let sql = format!("select to_jsonb(o)-'during' from porchlight.item_occurrences o
        join porchlight.items i on i.id=o.item_id
        cross join lateral (select
            least(o.starts_at at time zone 'UTC',o.due_at at time zone 'UTC',
                o.local_start,o.local_due,o.due_date::timestamp) as lo,
            greatest(o.starts_at at time zone 'UTC',o.ends_at at time zone 'UTC',
                o.due_at at time zone 'UTC',o.local_start,o.local_end,o.local_due,
                o.due_date::timestamp + interval '1 day') as hi) generated
        cross join lateral (select
            coalesce(o.override_local_start,o.local_start,o.starts_at at time zone 'UTC') as s,
            coalesce(o.override_local_end,o.local_end,o.ends_at at time zone 'UTC') as e,
            coalesce(o.override_local_due,o.local_due,o.due_at at time zone 'UTC') as d,
            coalesce(o.override_due_date,o.due_date)::timestamp as day) effective
        cross join lateral (select least(effective.s,effective.d,effective.day) as lo,
            greatest(effective.s,effective.e,effective.d,effective.day + interval '1 day') as hi) overridden
        where {VISIBLE} and o.household_id=$1 and (generated.lo is null or
            (generated.lo < ($5::timestamptz at time zone 'UTC') + interval '3 days'
             and generated.hi > ($4::timestamptz at time zone 'UTC') - interval '3 days') or
            (o.is_detached and overridden.lo < ($5::timestamptz at time zone 'UTC') + interval '3 days'
             and overridden.hi > ($4::timestamptz at time zone 'UTC') - interval '3 days'))
        order by o.item_id,o.seq");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(household)
        .bind(member)
        .bind(sees_all)
        .bind(from)
        .bind(to)
        .fetch_all(&mut **tx)
        .await?)
}
