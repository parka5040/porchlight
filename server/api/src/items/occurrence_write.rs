//! Occurrence status, overrides and board mutations.
use super::{
    acl::{authorize, domain_error, load, occurrence_response, occurrence_row, stale},
    validate::{field_decode, object, reference_error, validate_neighbour, validation},
    write::Operation,
};
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use porchlight_core::{access::Capability, items as core};
use porchlight_db::{
    derive,
    items::{BoardMove, ScopeRequest, WriteScope},
};
use serde_json::{Map, Value};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(super) async fn edit_occurrence(
    tx: &mut Transaction<'_, Postgres>,
    c: &Caller,
    id: Uuid,
    headers: &axum::http::HeaderMap,
    obj: &Map<String, Value>,
    op: Operation,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let old = occurrence_row(tx, c, id).await?;
    let parent = load(tx, c, old.item_id, false).await?;
    let overrides = [
        "override_local_start",
        "override_local_end",
        "override_local_due",
        "override_due_date",
    ];
    for name in obj.keys() {
        if !overrides.contains(&name.as_str())
            && !["status", "before_id", "after_id", "board_move"].contains(&name.as_str())
        {
            return Err(validation(name, "Unknown field"));
        }
    }
    if op != Operation::PatchOccurrence && !obj.is_empty() {
        return Err(validation("body", "Expected an empty object"));
    }
    let status = match op {
        Operation::Complete => Some(core::Status::Done),
        Operation::Reopen => Some(if old.status.is_open() {
            old.status
        } else {
            core::Status::Todo
        }),
        _ => obj
            .get("status")
            .map(|v| {
                let s: String = field_decode("status", v.clone())?;
                s.parse::<core::Status>()
                    .map_err(|e| validation(e.field, e.reason))
            })
            .transpose()?,
    };
    let has_override = overrides.iter().any(|k| obj.contains_key(*k));
    let board = obj.contains_key("board_move")
        || obj.contains_key("before_id")
        || obj.contains_key("after_id");
    let required = if op == Operation::Complete {
        Capability::Complete
    } else {
        Capability::ChangeStatus
    };
    authorize(
        tx,
        c,
        &parent,
        if has_override {
            Capability::Edit
        } else {
            required
        },
    )
    .await?;
    if has_override && (status.is_some() || board) {
        authorize(tx, c, &parent, required).await?;
    }
    let expected = crate::protocol::if_match(headers)?;
    let bounds = if let Some(v) = obj.get("board_move") {
        if obj.contains_key("before_id") || obj.contains_key("after_id") {
            return Err(validation(
                "board_move",
                "Use nested or direct bounds, not both",
            ));
        }
        let b = object(v)?;
        if b.keys().any(|k| k != "before_id" && k != "after_id") {
            return Err(validation("board_move", "Unknown bound"));
        }
        b
    } else {
        obj
    };
    // Wire after_id is predecessor; DB before is the predecessor (R30).
    let predecessor = bounds
        .get("after_id")
        .map(|v| field_decode::<Option<Uuid>>("after_id", v.clone()))
        .transpose()?
        .flatten();
    let successor = bounds
        .get("before_id")
        .map(|v| field_decode::<Option<Uuid>>("before_id", v.clone()))
        .transpose()?
        .flatten();
    if board && status.is_none_or(|s| s == old.status) {
        // R42: only unchanged-status moves use the current projection. Composed
        // transitions delegate mover membership to WriteScope's post-transition check.
        // Unlike a neighbour, an off-board mover is always the uniform R38 error.
        validate_neighbour(tx, c, Uuid::nil(), id, "board_move", old.status, now)
            .await
            .map_err(|e| {
                if e.problem() == Problem::ValidationFailed {
                    reference_error("board_move", "invalid-reference")
                } else {
                    e
                }
            })?;
    }
    for (field, neighbour) in [("after_id", predecessor), ("before_id", successor)] {
        if let Some(neighbour) = neighbour {
            validate_neighbour(
                tx,
                c,
                id,
                neighbour,
                field,
                status.unwrap_or(old.status),
                now,
            )
            .await?;
        }
    }
    let mut replacement = derive::effective_anchor(&old, &parent.spec().map_err(domain_error)?)
        .map_err(domain_error)?;
    for name in overrides {
        if let Some(v) = obj.get(name) {
            match name {
                "override_local_start" => replacement.local_start = field_decode(name, v.clone())?,
                "override_local_end" => replacement.local_end = field_decode(name, v.clone())?,
                "override_local_due" => replacement.local_due = field_decode(name, v.clone())?,
                _ => replacement.due_date = field_decode(name, v.clone())?,
            }
        }
    }
    if has_override {
        core::persisted_occurrence_override(
            &parent.spec().map_err(domain_error)?,
            old.anchor.seq,
            &replacement,
        )
        .map_err(|e| validation(e.field, e.reason))?;
    }
    if old.version != expected {
        return Err(stale(occurrence_response(tx, c, id, vec![]).await?));
    }
    if status.is_none() && !has_override && !board {
        return Err(validation("body", "No occurrence changes"));
    }
    let status = status.filter(|s| *s != old.status);
    if status.is_none() && !has_override && !board {
        return Ok(Json(occurrence_response(tx, c, id, vec![]).await?).into_response());
    }
    let authoritative = status.is_some() || has_override;
    let request = ScopeRequest {
        items: if authoritative {
            vec![parent.id]
        } else {
            vec![]
        },
        occurrences: if authoritative { vec![id] } else { vec![] },
        reopens: if status.is_some_and(|s| s.is_open()) && !old.status.is_open() {
            vec![id]
        } else {
            vec![]
        },
        board_moves: if board {
            vec![BoardMove {
                occurrence: id,
                before: predecessor,
                after: successor,
                status: status.unwrap_or(old.status),
            }]
        } else {
            vec![]
        },
        ..Default::default()
    };
    // A plan-time disappearance after bound validation is a retryable race.
    let board_plan_error = |e| match e {
        derive::Error::NotFound if board => ApiError::transient_conflict().retry(1),
        other => domain_error(other),
    };
    let mut scope = WriteScope::begin_at(tx, c.household_id(), request, now)
        .await
        .map_err(board_plan_error)?;
    let locked = scope.occurrence(id).await.map_err(domain_error)?;
    if locked.version != expected {
        drop(scope);
        return Err(stale(occurrence_response(tx, c, id, vec![]).await?));
    }
    if authoritative && scope.item(parent.id).await.map_err(domain_error)?.version != parent.version
    {
        return Err(ApiError::transient_conflict().retry(1));
    }
    let mut version = locked.version;
    if let Some(s) = status {
        scope
            .set_status(id, version, s, Some(c.member_id()), now)
            .await
            .map_err(domain_error)?;
        version = scope.occurrence(id).await.map_err(domain_error)?.version;
    }
    if has_override {
        scope
            .override_occurrence(id, version, &replacement)
            .await
            .map_err(domain_error)?;
        version = scope.occurrence(id).await.map_err(domain_error)?.version;
    }
    if board {
        scope
            .move_on_board(id, version, predecessor, successor)
            .await
            .map_err(|e| match e {
                // R42: missing mover/new-column membership, including disappearance
                // after an unchanged-status precheck, is deliberately a sealed
                // body-reference 422. Contention/stale neighbours remain distinct.
                derive::Error::NotFound => reference_error("board_move", "invalid-reference"),
                other => domain_error(other),
            })?;
    }
    let outcomes = scope.finish(now).await.map_err(domain_error)?;
    let raw = outcomes
        .into_iter()
        .flat_map(|o| o.derived.conflicts)
        .collect();
    // Board-only scopes intentionally retain no parent/member locks. Recheck
    // authoritative ACL after any wait, before committing effects or returning data.
    let current = load(tx, c, parent.id, false).await?;
    authorize(
        tx,
        c,
        &current,
        if has_override {
            Capability::Edit
        } else {
            required
        },
    )
    .await?;
    if has_override && (status.is_some() || board) {
        authorize(tx, c, &current, required).await?;
    }
    match occurrence_response(tx, c, id, raw).await {
        Ok(r) => Ok(Json(r).into_response()),
        // A reopened, rule-removed closed row may be deleted by regeneration.
        Err(e) if e.status() == StatusCode::NOT_FOUND => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => Err(e),
    }
}
