//! Read snapshots, batched collections and signed occurrence pages.
use super::{
    acl::{caps, domain_error, filter_capabilities, load, occurrence_wire, pending, wire},
    validate::validation,
};
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
    routes::items::{
        ConflictResponse, ItemPage, ItemResponse, ListQuery, OccurrencePage, OccurrencesQuery,
    },
    state::AppState,
};
use axum::http::StatusCode;
use porchlight_core::{
    access::{self, Capability},
    items as core,
};
use porchlight_db::{
    derive::{self, ItemRow},
    items::{self as db, ItemWrite, Relationships},
};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

pub async fn read_snapshot(s: &AppState) -> Result<Transaction<'static, Postgres>, ApiError> {
    let mut tx = porchlight_db::tx::begin(&s.pool).await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

pub(super) struct ReadBatch {
    rows: std::collections::BTreeMap<Uuid, ItemRow>,
    sets: std::collections::BTreeMap<Uuid, Relationships>,
    pending: std::collections::BTreeSet<Uuid>,
    dependent: std::collections::BTreeSet<Uuid>,
}
impl ReadBatch {
    pub(super) async fn load(
        conn: &mut PgConnection,
        household: Uuid,
        ids: &[Uuid],
    ) -> Result<Self, ApiError> {
        // Same interval projection as single-item reads (derive::load_item).
        let rows = derive::load_items(&mut *conn, ids)
            .await
            .map_err(domain_error)?;
        let mut sets: std::collections::BTreeMap<Uuid, Relationships> = ids
            .iter()
            .map(|id| (*id, Relationships::default()))
            .collect();
        for (query, assignees) in [
            (
                "SELECT item_id,member_id FROM porchlight.item_assignees WHERE item_id=ANY($1) ORDER BY member_id",
                true,
            ),
            (
                "SELECT item_id,member_id FROM porchlight.item_viewers WHERE item_id=ANY($1) ORDER BY member_id",
                false,
            ),
        ] {
            let pairs: Vec<(Uuid, Uuid)> = sqlx::query_as(query)
                .bind(ids)
                .fetch_all(&mut *conn)
                .await?;
            for (id, member) in pairs {
                let set = sets.get_mut(&id).unwrap();
                if assignees {
                    set.assignees.push(member)
                } else {
                    set.viewers.push(member)
                }
            }
        }
        let offsets: Vec<(Uuid,i32)> = sqlx::query_as("SELECT item_id,offset_minutes FROM porchlight.item_reminders WHERE item_id=ANY($1) ORDER BY offset_minutes").bind(ids).fetch_all(&mut *conn).await?;
        for (id, offset) in offsets {
            sets.get_mut(&id).unwrap().reminder_offsets.push(offset);
        }
        let subjects: Vec<Uuid> = rows.iter().filter_map(|r| r.subject_member_id).collect();
        let dependent: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM porchlight.household_members WHERE id=ANY($1) AND household_id=$2 AND role='dependent' AND deleted_at IS NULL").bind(subjects).bind(household).fetch_all(&mut *conn).await?;
        let pending: Vec<Uuid> = sqlx::query_scalar("SELECT item_id FROM porchlight.item_derivation WHERE item_id=ANY($1) AND requested_gen>applied_gen").bind(ids).fetch_all(conn).await?;
        Ok(Self {
            rows: rows.into_iter().map(|r| (r.id, r)).collect(),
            sets,
            pending: pending.into_iter().collect(),
            dependent: dependent.into_iter().collect(),
        })
    }
    fn caps(&self, c: &Caller, id: Uuid) -> Result<Vec<Capability>, ApiError> {
        let r = self
            .rows
            .get(&id)
            .ok_or_else(|| ApiError::new(Problem::NotFound))?;
        let ctx = super::acl::acl_context(
            c,
            r,
            &self.sets[&id],
            r.subject_member_id
                .is_some_and(|s| self.dependent.contains(&s)),
        )?;
        if !access::can_view(&ctx) {
            return Err(ApiError::new(Problem::NotFound));
        }
        Ok(filter_capabilities(
            r,
            c,
            access::capabilities(&ctx).iter().collect(),
        ))
    }
    fn item(
        &self,
        c: &Caller,
        id: Uuid,
        conflicts: Vec<ConflictResponse>,
    ) -> Result<ItemResponse, ApiError> {
        let caps = self.caps(c, id)?;
        let r = &self.rows[&id];
        let sets = &self.sets[&id];
        Ok(ItemResponse {
            id: r.id,
            household_id: r.household_id,
            version: r.version,
            fields: wire(r)?,
            assignees: sets.assignees.clone(),
            viewers: sets.viewers.clone(),
            reminder_offsets: sets.reminder_offsets.clone(),
            deleted_at: r.deleted_at,
            series_id: r.series_id,
            split_from_item_id: r.split_from_item_id,
            split_from_seq: r.split_from_seq,
            nominal_local_anchor: r.nominal_local_anchor,
            nominal_wall_duration: r.nominal_wall_duration,
            capabilities: caps.into_iter().map(|cap| cap.as_str().into()).collect(),
            pending_derivation: self.pending.contains(&id),
            conflicts,
        })
    }
    pub(super) fn mask(
        &self,
        c: &Caller,
        raw: derive::Conflict,
    ) -> Result<ConflictResponse, ApiError> {
        let visible = if let Some(id) = raw.other_item_id {
            if self.rows.get(&id).is_some_and(|r| r.deleted_at.is_none()) {
                match self.caps(c, id) {
                    Ok(_) => true,
                    Err(e) if e.status() == StatusCode::NOT_FOUND => false,
                    Err(e) => return Err(e),
                }
            } else {
                false
            }
        } else {
            false
        };
        Ok(super::acl::mask_conflict(raw, visible))
    }
}

pub async fn list(s: &AppState, c: &Caller, q: ListQuery) -> Result<ItemPage, ApiError> {
    let limit = crate::pagination::PageParams {
        cursor: q.cursor.clone(),
        limit: q.limit,
    }
    .limit()?;
    if q.include_deleted.unwrap_or(false) && !c.sees_all() {
        return Err(ApiError::new(Problem::Forbidden));
    }
    if let Some(kind) = &q.kind {
        kind.parse::<core::Kind>()
            .map_err(|e| validation(e.field, e.reason))?;
    }
    let context = format!(
        "items:{}:{}:{:?}:{:?}:{}:{}",
        c.household_id(),
        c.member_id(),
        q.updated_since,
        q.kind,
        q.include_deleted.unwrap_or(false),
        c.sees_all()
    );
    let cursor = q
        .cursor
        .as_ref()
        .map(|cursor| s.idem.cursor.decode::<Uuid>(&context, cursor))
        .transpose()?
        .unwrap_or(Uuid::nil());
    let mut conn = read_snapshot(s).await?;
    // ACL before LIMIT, never scan arbitrary invisible pages or trust derived rows.
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT i.id FROM porchlight.items i WHERE household_id=$1 AND id>$2 AND ($3 OR deleted_at IS NULL) AND ($4::timestamptz IS NULL OR updated_at>$4) AND ($5::text IS NULL OR kind=$5) AND ($6 OR porchlight.pl_item_can_read(i.id,$7)) ORDER BY id LIMIT $8")
        .bind(c.household_id()).bind(cursor).bind(q.include_deleted.unwrap_or(false)).bind(q.updated_since).bind(q.kind).bind(c.sees_all()).bind(c.member_id()).bind((limit+1) as i64).fetch_all(&mut *conn).await?;
    let page = s.idem.cursor.page(&context, ids, limit, |id| *id)?;
    let batch = ReadBatch::load(&mut conn, c.household_id(), &page.items).await?;
    let targets: Vec<_> = page
        .items
        .iter()
        .map(|id| {
            (
                *id,
                db::agenda_members(&ItemWrite::from(&batch.rows[id]), &batch.sets[id]),
            )
        })
        .collect();
    let conflicts = super::acl::batch_conflicts(&mut conn, &targets).await?;
    let counterpart_ids: Vec<_> = conflicts
        .iter()
        .filter_map(|(_, r)| r.0.other_item_id)
        .collect();
    let counterparts = ReadBatch::load(&mut conn, c.household_id(), &counterpart_ids).await?;
    let mut masked: std::collections::BTreeMap<Uuid, Vec<ConflictResponse>> =
        std::collections::BTreeMap::new();
    for (id, raw) in conflicts {
        masked
            .entry(id)
            .or_default()
            .push(counterparts.mask(c, raw.0)?);
    }
    let mut items = Vec::with_capacity(page.items.len());
    for id in page.items {
        match batch.item(c, id, masked.remove(&id).unwrap_or_default()) {
            Ok(item) => items.push(item),
            Err(e) if e.status() == StatusCode::NOT_FOUND => tracing::warn!(
                kind = "list_acl_rejected",
                "item collection row rejected by authoritative ACL"
            ),
            Err(e) => return Err(e),
        }
    }
    Ok(ItemPage {
        items,
        next_cursor: page.next_cursor,
    })
}
pub async fn occurrences(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    q: OccurrencesQuery,
) -> Result<OccurrencePage, ApiError> {
    occurrences_with_zone(s, c, id, q, None).await
}
/// The request's reported zone affects floating window filtering, never authority.
pub async fn occurrences_with_zone(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    q: OccurrencesQuery,
    reported_zone: Option<chrono_tz::Tz>,
) -> Result<OccurrencePage, ApiError> {
    if q.from.zip(q.to).is_some_and(|(f, t)| f >= t) {
        return Err(validation("to", "Must be after from"));
    }
    let mut conn = read_snapshot(s).await?;
    let parent = load(&mut conn, c, id, false).await?;
    let capabilities = caps(&mut conn, c, &parent)
        .await?
        .into_iter()
        .map(|cap| cap.as_str().into())
        .collect::<Vec<String>>();
    let is_pending = pending(&mut conn, id).await?;
    let spec = parent.spec().map_err(domain_error)?;
    let rows = derive::occurrences(&mut conn, id)
        .await
        .map_err(domain_error)?;
    let zone:String=sqlx::query_scalar("SELECT CASE WHEN m.role='dependent' THEN h.tzid ELSE m.tzid END FROM porchlight.household_members m JOIN porchlight.households h ON h.id=m.household_id WHERE m.id=$1").bind(c.member_id()).fetch_one(&mut *conn).await?;
    let zone = reported_zone
        .or_else(|| porchlight_core::time::parse_tz(&zone))
        .ok_or_else(|| ApiError::internal("member_zone"))?;
    let mut items = Vec::new();
    for o in rows {
        let r = occurrence_wire(&o, &spec, capabilities.clone(), is_pending, vec![])?;
        let start = r
            .starts_at
            .or(r.due_at)
            .or(r
                .local_start
                .or(r.local_due)
                .map(|l| porchlight_core::time::resolve_local(l, zone)))
            .or(r
                .due_date
                .map(|d| porchlight_core::time::start_of_day(d, zone)));
        let end = r
            .ends_at
            .or(r
                .local_end
                .map(|l| porchlight_core::time::resolve_local(l, zone)))
            .or(start);
        let in_range = match (start, end) {
            (Some(start), Some(end)) => {
                q.to.is_none_or(|t| start < t)
                    && q.from
                        .is_none_or(|f| if end == start { start >= f } else { end > f })
            }
            _ => q.undated == Some(true) || (q.from.is_none() && q.to.is_none()),
        };
        if in_range {
            items.push(r);
        }
    }
    items.sort_by_key(|r| r.seq);
    let limit = crate::pagination::PageParams {
        cursor: q.cursor.clone(),
        limit: q.limit,
    }
    .limit()?;
    let context = format!(
        "occurrences:{}:{}:{}:{}:{:?}:{:?}:{}:{}",
        c.household_id(),
        c.member_id(),
        id,
        c.sees_all(),
        q.from,
        q.to,
        zone,
        q.undated.unwrap_or(false)
    );
    let after = q
        .cursor
        .as_ref()
        .map(|cursor| s.idem.cursor.decode::<(u32, Uuid)>(&context, cursor))
        .transpose()?;
    items.retain(|r| after.is_none_or(|key| (r.seq, r.id) > key));
    let page = s
        .idem
        .cursor
        .page(&context, items, limit, |r| (r.seq, r.id))?;
    Ok(OccurrencePage {
        items: page.items,
        next_cursor: page.next_cursor,
    })
}
