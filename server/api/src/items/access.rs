//! Authoritative ACLs, capabilities and permission-aware wire responses.
use super::*;

pub fn domain_error(e: derive::Error) -> ApiError {
    match e {
        derive::Error::Sql(e) => e.into(),
        derive::Error::Validation(e) => validation(e.field, e.reason),
        derive::Error::NotFound => ApiError::new(Problem::NotFound),
        derive::Error::RetryableScope => ApiError::transient(Problem::Conflict).retry(1),
        derive::Error::Ordering(_) => validation("board_move", "Invalid board bounds"),
        // The mutation boundary rolls back, then attaches authorized current.
        derive::Error::VersionConflict => ApiError::new(Problem::VersionConflict),
        derive::Error::Agenda(_) => ApiError::internal("item_agenda"),
        _ => ApiError::internal("item_domain"),
    }
}
/// Attach an authorized current resource after rolling back domain effects.
/// `occurrence` distinguishes the path resource; hidden paths remain 404.
pub async fn attach_current(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    occurrence: bool,
    e: ApiError,
) -> ApiError {
    if e.problem() != Problem::VersionConflict {
        return e;
    }
    let current = if occurrence {
        occurrence_response(conn, c, id, vec![])
            .await
            .and_then(|r| {
                serde_json::to_value(r).map_err(|_| ApiError::internal("occurrence_encode"))
            })
    } else {
        response(conn, c, id, false, vec![])
            .await
            .and_then(|r| serde_json::to_value(r).map_err(|_| ApiError::internal("item_encode")))
    };
    match current {
        Ok(current) => e.extension("current", current),
        Err(e) => e,
    }
}
/// S8 replaces this single hook to remove write capabilities on feed-sourced items.
/// View/comment remain governed by the authoritative ACL; default is a no-op.
pub fn filter_capabilities(
    _row: &ItemRow,
    _caller: &Caller,
    capabilities: Vec<Capability>,
) -> Vec<Capability> {
    capabilities
}
pub(crate) async fn context(
    conn: &mut PgConnection,
    c: &Caller,
    r: &ItemRow,
) -> Result<AccessContext, ApiError> {
    if r.household_id != c.household_id() {
        return Err(ApiError::new(Problem::NotFound));
    }
    let sets = db::relationships(conn, r.id).await.map_err(domain_error)?;
    let dependent = if let Some(id) = r.subject_member_id {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM porchlight.household_members WHERE id=$1 AND household_id=$2 AND role='dependent' AND deleted_at IS NULL)").bind(id).bind(c.household_id()).fetch_one(&mut *conn).await?
    } else {
        false
    };
    Ok(AccessContext {
        caller: match c {
            Caller::Agent { .. } => CallerKind::Agent,
            Caller::Member { role, .. } => CallerKind::AppMember(
                role.parse::<Role>()
                    .map_err(|_| ApiError::new(Problem::Forbidden))?,
            ),
        },
        visibility: r
            .visibility
            .parse::<Visibility>()
            .map_err(|_| ApiError::internal("item_visibility"))?,
        is_owner: r.owner_id == c.member_id(),
        is_assignee: sets.assignees.contains(&c.member_id()),
        is_viewer: sets.viewers.contains(&c.member_id()),
        is_subject: r.subject_member_id == Some(c.member_id()),
        unassigned_dependent_item: dependent && sets.assignees.is_empty(),
    })
}
pub(crate) async fn caps(
    conn: &mut PgConnection,
    c: &Caller,
    r: &ItemRow,
) -> Result<Vec<Capability>, ApiError> {
    let ctx = context(conn, c, r).await?;
    if !access::can_view(&ctx) {
        return Err(ApiError::new(Problem::NotFound));
    }
    Ok(filter_capabilities(
        r,
        c,
        access::capabilities(&ctx).iter().collect(),
    ))
}
pub(crate) async fn authorize(
    conn: &mut PgConnection,
    c: &Caller,
    r: &ItemRow,
    cap: Capability,
) -> Result<(), ApiError> {
    if r.deleted_at.is_some() {
        return Err(ApiError::new(Problem::NotFound));
    }
    if !caps(conn, c, r).await?.contains(&cap) {
        return Err(ApiError::new(Problem::Forbidden));
    }
    // Agents hold every policy capability, but business eligibility is not waived.
    if cap == Capability::Take && !context(conn, c, r).await?.unassigned_dependent_item {
        return Err(ApiError::new(Problem::Forbidden));
    }
    Ok(())
}
pub(crate) async fn load(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    deleted: bool,
) -> Result<ItemRow, ApiError> {
    let r = derive::load_item(conn, id).await.map_err(domain_error)?;
    if !deleted && r.deleted_at.is_some() {
        return Err(ApiError::new(Problem::NotFound));
    }
    caps(conn, c, &r).await?;
    Ok(r)
}
pub(super) fn wire(r: &ItemRow) -> Result<ItemFields, ApiError> {
    serde_json::from_value(
        serde_json::to_value(ItemWrite::from(r)).map_err(|_| ApiError::internal("item_encode"))?,
    )
    .map_err(|_| ApiError::internal("item_decode"))
}
pub(crate) async fn pending(conn: &mut PgConnection, id: Uuid) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.item_derivation WHERE item_id=$1 AND requested_gen>applied_gen)").bind(id).fetch_one(conn).await?)
}
pub(crate) async fn mask(
    conn: &mut PgConnection,
    c: &Caller,
    raw: Vec<derive::Conflict>,
) -> Result<Vec<ConflictResponse>, ApiError> {
    let mut result = Vec::with_capacity(raw.len());
    for mut conflict in raw {
        let mut other_occurrence_id = Some(conflict.other_occurrence_id);
        if let Some(id) = conflict.other_item_id {
            match load(conn, c, id, false).await {
                Ok(_) => {}
                Err(e) if e.status() == StatusCode::NOT_FOUND => {
                    conflict.other_item_id = None;
                    other_occurrence_id = None;
                    conflict.other_title = "Busy".into();
                }
                Err(e) => return Err(e),
            }
        } else {
            conflict.other_title = "Busy".into();
            other_occurrence_id = None;
        }
        result.push(ConflictResponse {
            member_id: conflict.member_id,
            occurrence_id: conflict.occurrence_id,
            other_occurrence_id,
            other_item_id: conflict.other_item_id,
            other_title: conflict.other_title,
            starts_at: conflict.starts_at,
            ends_at: conflict.ends_at,
            kind: conflict.kind,
        });
    }
    Ok(result)
}
pub async fn response(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    deleted: bool,
    raw: Vec<derive::Conflict>,
) -> Result<ItemResponse, ApiError> {
    let r = load(conn, c, id, deleted).await?;
    let capabilities = caps(conn, c, &r)
        .await?
        .into_iter()
        .map(|cap| cap.as_str().into())
        .collect();
    let sets = db::relationships(conn, id).await.map_err(domain_error)?;
    let raw = if raw.is_empty() {
        derive::conflicts(conn, id, &db::agenda_members(&ItemWrite::from(&r), &sets))
            .await
            .map_err(domain_error)?
    } else {
        raw
    };
    Ok(ItemResponse {
        id: r.id,
        household_id: r.household_id,
        version: r.version,
        fields: wire(&r)?,
        assignees: sets.assignees,
        viewers: sets.viewers,
        reminder_offsets: sets.reminder_offsets,
        deleted_at: r.deleted_at,
        series_id: r.series_id,
        split_from_item_id: r.split_from_item_id,
        split_from_seq: r.split_from_seq,
        nominal_local_anchor: r.nominal_local_anchor,
        nominal_wall_duration: r.nominal_wall_duration,
        capabilities,
        pending_derivation: pending(conn, id).await?,
        conflicts: mask(conn, c, raw).await?,
    })
}
pub(crate) async fn occurrence_row(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
) -> Result<OccurrenceRow, ApiError> {
    let SqlJson(row) = sqlx::query_scalar::<_, SqlJson<OccurrenceRow>>(
        "SELECT to_jsonb(o) FROM porchlight.item_occurrences o WHERE id=$1 AND household_id=$2",
    )
    .bind(id)
    .bind(c.household_id())
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| ApiError::new(Problem::NotFound))?;
    Ok(row)
}
pub(crate) async fn occurrence_response(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    raw: Vec<derive::Conflict>,
) -> Result<OccurrenceResponse, ApiError> {
    let o = occurrence_row(conn, c, id).await?;
    let r = load(conn, c, o.item_id, false).await?;
    let capabilities = caps(conn, c, &r)
        .await?
        .into_iter()
        .map(|cap| cap.as_str().into())
        .collect();
    let is_pending = pending(conn, r.id).await?;
    let conflicts = mask(conn, c, raw).await?;
    occurrence_wire(
        &o,
        &r.spec().map_err(domain_error)?,
        capabilities,
        is_pending,
        conflicts,
    )
}
pub(super) fn occurrence_wire(
    o: &OccurrenceRow,
    spec: &core::SeriesSpec,
    capabilities: Vec<String>,
    is_pending: bool,
    conflicts: Vec<ConflictResponse>,
) -> Result<OccurrenceResponse, ApiError> {
    let a = derive::effective_anchor(o, spec).map_err(domain_error)?;
    Ok(OccurrenceResponse {
        id: o.id,
        item_id: o.item_id,
        seq: a.seq,
        version: o.version,
        status: o.status.as_str().into(),
        disposition: o.disposition.as_str().into(),
        board_position: o.board_position.clone(),
        completed_by: o.completed_by,
        completed_at: o.completed_at,
        local_start: a.local_start,
        local_end: a.local_end,
        local_due: a.local_due,
        starts_at: a.starts_at,
        ends_at: a.ends_at,
        due_at: a.due_at,
        due_date: a.due_date,
        override_local_start: o.override_local_start,
        override_local_end: o.override_local_end,
        override_local_due: o.override_local_due,
        override_due_date: o.override_due_date,
        is_detached: o.is_detached,
        capabilities,
        pending_derivation: is_pending,
        conflicts,
    })
}
pub(super) fn stale(current: impl serde::Serialize) -> ApiError {
    ApiError::new(Problem::VersionConflict).extension(
        "current",
        serde_json::to_value(current).expect("wire serialization"),
    )
}
