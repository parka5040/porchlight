//! Authoritative ACLs, capabilities and permission-aware wire responses.
use super::{read::ReadBatch, validate::validation};
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
    routes::items::{ConflictResponse, ItemFields, ItemResponse, OccurrenceResponse},
};
use porchlight_core::{
    access::{self, AccessContext, CallerKind, Capability, Role, Visibility},
    items as core,
};
use porchlight_db::{
    derive::{self, ItemRow, OccurrenceRow},
    items::{self as db, ItemWrite, Relationships},
};
use sqlx::{PgConnection, types::Json as SqlJson};
use uuid::Uuid;

pub fn domain_error(e: derive::Error) -> ApiError {
    match e {
        derive::Error::Sql(e) => e.into(),
        derive::Error::Validation(e) => validation(e.field, e.reason),
        derive::Error::NotFound => ApiError::new(Problem::NotFound),
        derive::Error::RetryableScope => ApiError::transient_conflict().retry(1),
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
/// Single feed-read-only rule for both typed item reads/writes and JSON projections.
/// The authoritative marker restricts app capabilities; agents retain their ACL rights.
pub fn restrict_feed(
    has_feed_marker: bool,
    caller: &Caller,
    capabilities: Vec<Capability>,
) -> Vec<Capability> {
    if has_feed_marker && !caller.sees_all() {
        capabilities
            .into_iter()
            .filter(|cap| matches!(cap, Capability::View | Capability::Comment))
            .collect()
    } else {
        capabilities
    }
}
pub fn filter_capabilities(
    row: &ItemRow,
    caller: &Caller,
    capabilities: Vec<Capability>,
) -> Vec<Capability> {
    restrict_feed(row.ics_source_id.is_some(), caller, capabilities)
}
pub(super) async fn context(
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
    acl_context(c, r, &sets, dependent)
}
/// The only authoritative ACL context builder, shared by single reads and batches.
pub(super) fn acl_context(
    c: &Caller,
    r: &ItemRow,
    sets: &Relationships,
    dependent: bool,
) -> Result<AccessContext, ApiError> {
    if r.household_id != c.household_id() {
        return Err(ApiError::new(Problem::NotFound));
    }
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

pub(super) async fn caps(
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
pub(super) async fn pending(conn: &mut PgConnection, id: Uuid) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.item_derivation WHERE item_id=$1 AND requested_gen>applied_gen)").bind(id).fetch_one(conn).await?)
}
pub(crate) async fn mask(
    conn: &mut PgConnection,
    c: &Caller,
    raw: Vec<derive::Conflict>,
) -> Result<Vec<ConflictResponse>, ApiError> {
    let ids: Vec<_> = raw.iter().filter_map(|r| r.other_item_id).collect();
    let batch = ReadBatch::load(conn, c.household_id(), &ids).await?;
    raw.into_iter().map(|r| batch.mask(c, r)).collect()
}
/// One masking implementation: hidden counterparts expose neither identifier.
pub(super) fn mask_conflict(raw: derive::Conflict, visible: bool) -> ConflictResponse {
    ConflictResponse {
        member_id: raw.member_id,
        occurrence_id: raw.occurrence_id,
        other_occurrence_id: visible.then_some(raw.other_occurrence_id),
        other_item_id: raw.other_item_id.filter(|_| visible),
        other_title: if visible {
            raw.other_title
        } else {
            "Busy".into()
        },
        starts_at: raw.starts_at,
        ends_at: raw.ends_at,
        kind: raw.kind,
    }
}
/// DB-side batch, with the same authoritative agenda-member set as derive::conflicts.
/// Kept here because the concurrent workstream owns DB/shared files this round.
pub(super) async fn batch_conflicts(
    conn: &mut PgConnection,
    targets: &[(Uuid, Vec<Uuid>)],
) -> Result<Vec<(Uuid, SqlJson<derive::Conflict>)>, ApiError> {
    let mut ids = Vec::new();
    let mut members = Vec::new();
    for (id, agenda_members) in targets {
        for member in agenda_members {
            ids.push(*id);
            members.push(*member);
        }
    }
    let mut result:Vec<(Uuid,SqlJson<derive::Conflict>)>=sqlx::query_as("SELECT c.item_id,to_jsonb(c)-'item_id' FROM (SELECT a.item_id,a.member_id,a.occurrence_id,b.occurrence_id AS other_occurrence_id,b.item_id AS other_item_id,i.title AS other_title,greatest(lower(a.busy),lower(b.busy)) AS starts_at,least(upper(a.busy),upper(b.busy)) AS ends_at,'hard'::text AS kind FROM unnest($1::uuid[],$2::uuid[]) AS target(item_id,member_id) JOIN porchlight.member_agenda a ON a.item_id=target.item_id AND a.member_id=target.member_id JOIN porchlight.member_agenda b ON b.member_id=a.member_id AND b.busy && a.busy AND b.occurrence_id<>a.occurrence_id JOIN porchlight.items i ON i.id=b.item_id AND i.deleted_at IS NULL) c ORDER BY c.member_id,c.occurrence_id,c.other_occurrence_id").bind(ids).bind(members).fetch_all(&mut *conn).await?;
    let item_ids: Vec<_> = targets.iter().map(|(id, _)| *id).collect();
    result.extend(
        porchlight_db::planning::load_conflicts(conn, &item_ids, None)
            .await
            .map_err(domain_error)?
            .into_iter()
            .filter(|(item, c)| {
                targets
                    .iter()
                    .any(|(id, members)| id == item && members.contains(&c.member_id))
            })
            .map(|(id, c)| (id, SqlJson(c))),
    );
    Ok(result)
}

/// Shared authoritative item ACL/capability check for item-attached resources.
pub async fn resource_capabilities(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
) -> Result<Vec<Capability>, ApiError> {
    let r = derive::load_item(conn, id).await.map_err(domain_error)?;
    if r.deleted_at.is_some() {
        return Err(ApiError::new(Problem::NotFound));
    }
    caps(conn, c, &r).await
}

/// Compatibility entry point for GET routes; the historical vector argument is unused.
/// Mutations use response_with_conflicts with an explicit Some (including empty).
pub async fn response(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    deleted: bool,
    _raw: Vec<derive::Conflict>,
) -> Result<ItemResponse, ApiError> {
    response_with_conflicts(conn, c, id, deleted, None).await
}
pub(super) async fn response_with_conflicts(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    deleted: bool,
    raw: Option<Vec<derive::Conflict>>,
) -> Result<ItemResponse, ApiError> {
    let r = load(conn, c, id, deleted).await?;
    let capabilities = caps(conn, c, &r)
        .await?
        .into_iter()
        .map(|cap| cap.as_str().into())
        .collect();
    let sets = db::relationships(conn, id).await.map_err(domain_error)?;
    let raw = match raw {
        Some(raw) => raw,
        None => batch_conflicts(
            conn,
            &[(id, db::agenda_members(&ItemWrite::from(&r), &sets))],
        )
        .await?
        .into_iter()
        .map(|(_, r)| r.0)
        .collect(),
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
pub(super) async fn occurrence_row(
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
pub(super) async fn occurrence_response(
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
