//! Wire-field validation and uniform body-reference errors.
use super::*;

pub(super) fn validation(field: &str, message: impl Into<String>) -> ApiError {
    ApiError::validation(vec![FieldError {
        code: None,
        field: field.into(),
        message: message.into(),
    }])
}
pub(crate) fn reference_error(field: &str, code: &str) -> ApiError {
    ApiError::validation(vec![FieldError {
        field: field.into(),
        message: "Invalid reference".into(),
        code: Some(code.into()),
    }])
}
pub(super) async fn validate_neighbour(
    conn: &mut PgConnection,
    c: &Caller,
    mover: Uuid,
    id: Uuid,
    field: &str,
    status: core::Status,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let invalid = || reference_error(field, "invalid-reference");
    let o = occurrence_row(conn, c, id).await.map_err(|e| {
        if e.status() == StatusCode::NOT_FOUND {
            invalid()
        } else {
            e
        }
    })?;
    load(conn, c, o.item_id, false).await.map_err(|e| {
        if e.status() == StatusCode::NOT_FOUND {
            invalid()
        } else {
            e
        }
    })?;
    if id == mover || o.status != status {
        return Err(invalid());
    }
    let projected = porchlight_db::items::is_board_card(conn, c.household_id(), id, now)
        .await
        .map_err(domain_error)?;
    if !projected {
        return Err(reference_error(field, "stale-neighbour"));
    }
    Ok(())
}
pub(super) fn object(v: &Value) -> Result<&Map<String, Value>, ApiError> {
    v.as_object()
        .ok_or_else(|| validation("body", "Expected an object"))
}
pub(crate) fn field_decode<T: serde::de::DeserializeOwned>(
    name: &str,
    v: Value,
) -> Result<T, ApiError> {
    serde_json::from_value(v).map_err(|_| validation(name, "Invalid value"))
}
pub(super) fn apply(base: &ItemWrite, patch: &Map<String, Value>) -> Result<ItemWrite, ApiError> {
    let mut value = serde_json::to_value(base).map_err(|_| ApiError::internal("item_encode"))?;
    let obj = value.as_object_mut().unwrap();
    for (k, v) in patch {
        if k == "scope" || k == "occurrence_id" {
            continue;
        }
        if !obj.contains_key(k) {
            return Err(validation(k, "Unknown field"));
        }
        // Validate each field independently to retain wire field names for type errors.
        let mut single = serde_json::to_value(base).unwrap();
        single[k] = v.clone();
        field_decode::<ItemWrite>(k, single)?;
        obj.insert(k.clone(), v.clone());
    }
    field_decode("body", value)
}
pub(super) fn validate_fields(f: &ItemWrite) -> Result<(), ApiError> {
    if !(1..=200).contains(&f.title.trim_matches(' ').chars().count()) {
        return Err(validation("title", "Must contain 1–200 characters"));
    }
    if !(0..=3).contains(&f.priority) {
        return Err(validation("priority", "Must be between 0 and 3"));
    }
    for (name, n) in [
        ("buffer_before_min", f.buffer_before_min),
        ("buffer_after_min", f.buffer_after_min),
    ] {
        if !(0..=1440).contains(&n) {
            return Err(validation(name, "Must be between 0 and 1440"));
        }
    }
    if f.est_minutes.is_some_and(|n| !(1..=10080).contains(&n)) {
        return Err(validation("est_minutes", "Must be between 1 and 10080"));
    }
    for (field, text, max) in [
        ("notes", Some(f.notes.as_str()), 20000),
        ("address", f.address.as_deref(), 500),
        ("source", f.source.as_deref(), 200),
    ] {
        if text.is_some_and(|s| s.chars().count() > max) {
            return Err(validation(
                field,
                format!("Must contain at most {max} characters"),
            ));
        }
    }
    f.visibility
        .parse::<Visibility>()
        .map_err(|_| validation("visibility", "Invalid visibility"))?;
    Ok(())
}
pub(super) async fn validate_members(
    conn: &mut PgConnection,
    c: &Caller,
    f: &ItemWrite,
    sets: &Relationships,
) -> Result<(), ApiError> {
    let mut checks = vec![
        ("owner_id", vec![f.owner_id]),
        ("assignees", sets.assignees.clone()),
        ("viewers", sets.viewers.clone()),
    ];
    if let Some(id) = f.subject_member_id {
        checks.push(("subject_member_id", vec![id]));
    }
    for (name, ids) in checks {
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM porchlight.household_members WHERE household_id=$1 AND id=ANY($2) AND deleted_at IS NULL").bind(c.household_id()).bind(&ids).fetch_one(&mut *conn).await?;
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        if unique.len() != ids.len() || count as usize != ids.len() {
            return Err(validation(
                name,
                "Expected unique live members of this household",
            ));
        }
    }
    let offsets: std::collections::BTreeSet<_> = sets.reminder_offsets.iter().collect();
    if offsets.len() != sets.reminder_offsets.len()
        || sets
            .reminder_offsets
            .iter()
            .any(|n| !(0..=40320).contains(n))
    {
        return Err(validation(
            "reminder_offsets",
            "Expected unique offsets between 0 and 40320",
        ));
    }
    Ok(())
}
