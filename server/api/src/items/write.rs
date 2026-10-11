//! Savepoint orchestration and retained domain scopes.
use super::{
    acl::{
        attach_current, authorize, domain_error, load, occurrence_row, response,
        response_with_conflicts, stale,
    },
    occurrence_write::edit_occurrence,
    validate::{
        apply, field_decode, object, reference_error, validate_fields, validate_members, validation,
    },
};
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
    idempotency::IdemTx,
    routes::items::EditQuery,
};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use porchlight_core::{access::Capability, items as core};
use porchlight_db::items::{self as db, ItemWrite, Relationships, ScopeRequest, WriteScope};
use serde_json::Value;
use sqlx::{Acquire, Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Create,
    Patch,
    Delete,
    Assignees,
    Viewers,
    Reminders,
    Take,
    PatchOccurrence,
    Complete,
    Reopen,
}
pub(super) fn target(tx: &IdemTx) -> Result<Uuid, ApiError> {
    tx.context
        .target_id
        .ok_or_else(|| ApiError::new(Problem::InvalidRequest))
}
/// All domain effects, including failed derivation, roll back before S1 stores business errors.
pub async fn mutate(
    c: Caller,
    tx: &mut IdemTx,
    mut body: Value,
    op: Operation,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    if op == Operation::Patch {
        let axum::extract::Query(q) =
            axum::extract::Query::<EditQuery>::try_from_uri(&tx.context.uri)
                .map_err(|_| ApiError::new(Problem::InvalidRequest))?;
        let patch = body
            .as_object_mut()
            .ok_or_else(|| validation("body", "Expected an object"))?;
        for (name, value) in [
            ("scope", q.scope.map(Value::String)),
            (
                "occurrence_id",
                q.occurrence_id.map(|id| Value::String(id.to_string())),
            ),
        ] {
            if let Some(value) = value {
                if patch.get(name).is_some_and(|v| v != &value) {
                    return Err(validation(name, "Conflicting body and query values"));
                }
                patch.insert(name.into(), value);
            }
        }
    }
    let id = if op == Operation::Create {
        Uuid::nil()
    } else {
        target(tx)?
    };
    let headers = tx.context.if_match.clone();
    let mut domain = tx.tx.begin().await?;
    let result = execute(&mut domain, &c, id, &headers, body, op, now).await;
    match result {
        Ok(r) => {
            domain.commit().await?;
            Ok(r)
        }
        Err(e) => {
            domain.rollback().await?;
            Err(attach_current(
                &mut tx.tx,
                &c,
                id,
                matches!(
                    op,
                    Operation::PatchOccurrence | Operation::Complete | Operation::Reopen
                ),
                e,
            )
            .await)
        }
    }
}
pub(super) async fn execute(
    tx: &mut Transaction<'_, Postgres>,
    c: &Caller,
    id: Uuid,
    headers: &axum::http::HeaderMap,
    body: Value,
    op: Operation,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let obj = object(&body)?;
    if matches!(
        op,
        Operation::PatchOccurrence | Operation::Complete | Operation::Reopen
    ) {
        return edit_occurrence(tx, c, id, headers, obj, op, now).await;
    }
    let mut sets = Relationships::default();
    if op == Operation::Create {
        let mut patch = obj.clone();
        for name in ["assignees", "viewers", "reminder_offsets"] {
            if let Some(v) = patch.remove(name) {
                match name {
                    "assignees" => sets.assignees = field_decode(name, v)?,
                    "viewers" => sets.viewers = field_decode(name, v)?,
                    _ => sets.reminder_offsets = field_decode(name, v)?,
                }
            }
        }
        if patch.contains_key("scope") || patch.contains_key("occurrence_id") {
            return Err(validation("scope", "Not valid for create"));
        }
        let fields = apply(
            &ItemWrite {
                owner_id: c.member_id(),
                ..Default::default()
            },
            &patch,
        )?;
        validate_fields(&fields)?;
        let spec = core::SeriesSpec::new(fields.timing.clone())
            .map_err(|e| validation(e.field, e.reason))?;
        validate_materialization(tx, c, &spec, now).await?;
        validate_members(tx, c, &fields, &sets).await?;
        if !c.sees_all() && fields.owner_id != c.member_id() {
            return Err(ApiError::new(Problem::Forbidden));
        }
        let request = ScopeRequest {
            members: db::agenda_members(&fields, &sets),
            ..Default::default()
        };
        let mut scope = WriteScope::begin_at(tx, c.household_id(), request, now)
            .await
            .map_err(domain_error)?;
        let id = scope.create(&fields, &sets).await.map_err(domain_error)?;
        let outcomes = scope.finish(now).await.map_err(domain_error)?;
        let raw = outcomes
            .into_iter()
            .flat_map(|o| o.derived.conflicts)
            .collect();
        return Ok((
            StatusCode::CREATED,
            Json(response_with_conflicts(tx, c, id, false, Some(raw)).await?),
        )
            .into_response());
    }
    let old = load(tx, c, id, false).await?;
    let required = match op {
        Operation::Patch => Capability::Edit,
        Operation::Delete => Capability::Delete,
        Operation::Assignees => Capability::Reassign,
        Operation::Viewers => Capability::ChangeVisibility,
        Operation::Reminders => Capability::EditReminders,
        Operation::Take => Capability::Take,
        _ => unreachable!(),
    };
    authorize(tx, c, &old, required).await?;
    let expected = crate::protocol::if_match(headers)?;
    if old.version != expected {
        return Err(stale(response(tx, c, id, false, vec![]).await?));
    }
    sets = db::relationships(tx, id).await.map_err(domain_error)?;
    let mut fields = if op == Operation::Patch {
        apply(&ItemWrite::from(&old), obj)?
    } else {
        ItemWrite::from(&old)
    };
    if op == Operation::Patch {
        if fields.owner_id != old.owner_id {
            authorize(tx, c, &old, Capability::Reassign).await?;
        }
        if fields.visibility != old.visibility {
            authorize(tx, c, &old, Capability::ChangeVisibility).await?;
        }
        validate_fields(&fields)?;
    } else {
        let name = match op {
            Operation::Assignees => Some("assignees"),
            Operation::Viewers => Some("viewers"),
            Operation::Reminders => Some("reminder_offsets"),
            _ => None,
        };
        if let Some(name) = name {
            if obj.len() != 1 || !obj.contains_key(name) {
                return Err(validation(name, "Expected whole-set replacement"));
            }
            match op {
                Operation::Assignees => sets.assignees = field_decode(name, obj[name].clone())?,
                Operation::Viewers => sets.viewers = field_decode(name, obj[name].clone())?,
                _ => sets.reminder_offsets = field_decode(name, obj[name].clone())?,
            }
        } else if !obj.is_empty() {
            return Err(validation("body", "Expected an empty object"));
        }
        if op == Operation::Take {
            sets.assignees.push(c.member_id());
        }
    }
    validate_members(tx, c, &fields, &sets).await?;
    let scope_name = obj
        .get("scope")
        .map(|v| field_decode::<String>("scope", v.clone()))
        .transpose()?
        .unwrap_or_else(|| "all".into());
    if scope_name != "all" && scope_name != "this_and_future" {
        return Err(validation("scope", "Expected all or this_and_future"));
    }
    let split = if scope_name == "this_and_future" {
        if old.timing.recurrence_mode.is_none() {
            return Err(validation("scope", "Requires a recurring item"));
        }
        let boundary: Uuid = field_decode(
            "occurrence_id",
            obj.get("occurrence_id").cloned().unwrap_or(Value::Null),
        )?;
        let occurrence = occurrence_row(tx, c, boundary).await.map_err(|e| {
            if e.status() == StatusCode::NOT_FOUND {
                reference_error("occurrence_id", "invalid-reference")
            } else {
                e
            }
        })?;
        if occurrence.item_id != id {
            return Err(reference_error("occurrence_id", "invalid-reference"));
        }
        Some(occurrence.anchor.seq)
    } else {
        if obj.contains_key("occurrence_id") {
            return Err(validation(
                "occurrence_id",
                "Requires this_and_future scope",
            ));
        }
        None
    };
    if let Some(seq) = split {
        let (_, tail) = core::this_and_future(&old.spec().map_err(domain_error)?, seq)
            .map_err(|e| validation(e.field, e.reason))?;
        let mut base = ItemWrite::from(&old);
        base.timing = tail.input().clone();
        fields = apply(&base, obj)?;
    }
    if op == Operation::Patch {
        validate_fields(&fields)?;
        let spec = if split.is_none() && fields.timing == old.timing {
            old.spec().map_err(domain_error)?
        } else {
            core::SeriesSpec::new(fields.timing.clone())
                .map_err(|e| validation(e.field, e.reason))?
        };
        validate_materialization(tx, c, &spec, now).await?;
    }
    let request = ScopeRequest {
        items: vec![id],
        members: db::agenda_members(&fields, &sets),
        splits: split
            .filter(|seq| *seq > 0)
            .map(|seq| vec![(id, seq)])
            .unwrap_or_default(),
        ..Default::default()
    };
    let mut scope = WriteScope::begin_at(tx, c.household_id(), request, now)
        .await
        .map_err(domain_error)?;
    let locked = scope.item(id).await.map_err(domain_error)?;
    if locked.version != expected || old.version != expected {
        drop(scope);
        return Err(stale(response(tx, c, id, false, vec![]).await?));
    }
    // Parent versions cover ACL changes too. Equality with the authorized planning
    // snapshot makes its ACL authoritative under the retained item lock.
    let mut result_id = id;
    if op == Operation::Patch {
        if let Some(seq) = split {
            let (_, tail) = scope.split(id, expected, seq).await.map_err(domain_error)?;
            result_id = tail;
            let tail_row = scope.item(tail).await.map_err(domain_error)?;
            let tail_fields = apply(&ItemWrite::from(&tail_row), obj)?;
            validate_fields(&tail_fields)?;
            scope
                .update(tail, tail_row.version, &tail_fields)
                .await
                .map_err(domain_error)?;
        } else {
            scope
                .update(id, expected, &fields)
                .await
                .map_err(domain_error)?;
        }
    } else if op == Operation::Delete {
        scope
            .soft_delete(id, expected, now)
            .await
            .map_err(domain_error)?;
    } else {
        scope
            .replace_relationships(id, expected, &sets)
            .await
            .map_err(domain_error)?;
    }
    let outcomes = scope.finish(now).await.map_err(domain_error)?;
    let raw = outcomes
        .into_iter()
        .flat_map(|o| o.derived.conflicts)
        .collect();
    match response_with_conflicts(tx, c, result_id, op == Operation::Delete, Some(raw)).await {
        Ok(r) => Ok(Json(r).into_response()),
        // A transfer/removal can legitimately evict the writer. Do not disclose
        // the now-private resource; the successful mutation remains committed.
        Err(e) if e.status() == StatusCode::NOT_FOUND => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => Err(e),
    }
}

async fn validate_materialization(
    tx: &mut Transaction<'_, Postgres>,
    c: &Caller,
    spec: &core::SeriesSpec,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    if spec.input().recurrence_mode != Some(core::RecurrenceMode::Calendar) {
        return Ok(());
    }
    let zone = match spec.zone() {
        Some(zone) => zone,
        None => {
            let name: String =
                sqlx::query_scalar("select tzid from porchlight.households where id=$1")
                    .bind(c.household_id())
                    .fetch_one(&mut **tx)
                    .await?;
            porchlight_core::time::parse_tz(&name)
                .ok_or_else(|| ApiError::internal("household_zone"))?
        }
    };
    let until = porchlight_core::time::to_local(
        core::horizon_end(now, zone).map_err(|e| validation(e.field, e.reason))?,
        zone,
    );
    let count = core::bounded_occurrence_count(spec, until, core::MAX_MATERIALIZED_OCCURRENCES)
        .map_err(|e| validation(e.field, e.reason))?;
    if count > core::MAX_MATERIALIZED_OCCURRENCES {
        return Err(validation(
            "rrule",
            "Series exceeds the 10000-occurrence materialization budget",
        ));
    }
    Ok(())
}
