use super::{
    acl::{authorize, load, response_with_conflicts},
    validate::{field_decode, reference_error, validate_fields, validate_members},
};
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
    idempotency::IdemTx,
};
use axum::{
    Json,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use porchlight_core::access::Capability;
use porchlight_db::items::{self as db, ItemWrite, Relationships, ScopeRequest, WriteScope};
use serde_json::Value;
use sqlx::Acquire;
use uuid::Uuid;

pub async fn revert(
    c: Caller,
    tx: &mut IdemTx,
    revision: Uuid,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let id = tx
        .context
        .target_id
        .ok_or_else(|| ApiError::new(Problem::InvalidRequest))?;
    let version = crate::protocol::if_match(&tx.context.if_match)?;
    let mut domain = tx.tx.begin().await?;
    let result = async {
        let old = load(&mut domain, &c, id, true).await?;
        // Revert is the only agent write that can restore a deleted path resource.
        // Reuse the ordinary capability check against its unchanged authoritative ACL.
        let mut authorized = old.clone();
        authorized.deleted_at = None;
        authorize(&mut domain, &c, &authorized, Capability::Edit).await?;
        if old.version != version {
            return Err(ApiError::new(Problem::VersionConflict).extension(
                "current",
                serde_json::to_value(super::response(&mut domain, &c, id, true, vec![]).await?)
                    .unwrap(),
            ));
        }
        let snapshot: Value = sqlx::query_scalar(
            "select snapshot from porchlight.item_revisions where
household_id=$1 and item_id=$2 and id=$3",
        )
        .bind(c.household_id())
        .bind(id)
        .bind(revision)
        .fetch_optional(&mut *domain)
        .await?
        .ok_or_else(|| reference_error("revision_id", "invalid-reference"))?;
        let mut fields = serde_json::to_value(ItemWrite::default()).unwrap();
        for (k, v) in fields.as_object_mut().unwrap() {
            *v = snapshot.get(k).cloned().unwrap_or(Value::Null);
        }
        fields["completion_interval"] = sqlx::query_scalar::<_, Value>(
            "select
coalesce(porchlight.pl_interval_json(($1::jsonb->>'completion_interval')::interval),'null'::jsonb)",
        )
        .bind(&snapshot)
        .fetch_one(&mut *domain)
        .await?;
        let fields: ItemWrite = field_decode("revision_id", fields)?;
        let sets = Relationships {
            assignees: field_decode("revision_id", snapshot["assignees"].clone())?,
            viewers: field_decode("revision_id", snapshot["viewers"].clone())?,
            reminder_offsets: field_decode("revision_id", snapshot["reminders"].clone())?,
        };
        validate_fields(&fields)?;
        validate_members(&mut domain, &c, &fields, &sets).await?;
        let scope_request = ScopeRequest {
            items: vec![id],
            members: db::agenda_members(&fields, &sets),
            ..Default::default()
        };
        let mut scope = WriteScope::begin_at(&mut domain, c.household_id(), scope_request, now)
            .await
            .map_err(super::domain_error)?;
        if scope.item(id).await.map_err(super::domain_error)?.version != version {
            drop(scope);
            return Err(ApiError::new(Problem::VersionConflict).extension(
                "current",
                serde_json::to_value(super::response(&mut domain, &c, id, true, vec![]).await?)
                    .unwrap(),
            ));
        }
        scope
            .restore_snapshot(id, version, &fields, &sets, &snapshot)
            .await
            .map_err(super::domain_error)?;
        let raw = scope
            .finish(now)
            .await
            .map_err(super::domain_error)?
            .into_iter()
            .flat_map(|o| o.derived.conflicts)
            .collect();
        Ok(
            Json(response_with_conflicts(&mut domain, &c, id, true, Some(raw)).await?)
                .into_response(),
        )
    }
    .await;
    match result {
        Ok(r) => {
            domain.commit().await?;
            Ok(r)
        }
        Err(e) => {
            domain.rollback().await?;
            Err(e)
        }
    }
}
