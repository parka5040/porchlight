//! List domain service. SQL always uses the caller's connection; no item derivation inputs change.
use crate::{
    error::{ApiError, FieldError, Problem},
    extract::Caller,
    idempotency::IdemTx,
    items::{field_decode, reference_error},
    list_types::*,
    state::AppState,
};
use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use porchlight_core::{
    access::{self, CallerKind, Capability, ListCapability, Role, Visibility},
    ordering,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::{Acquire, PgConnection};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq)]
pub enum Operation {
    Create,
    Patch,
    Delete,
    AddEntry,
    PatchEntry,
    DeleteEntry,
    Check,
    Uncheck,
    Clear,
    Viewers,
}
fn invalid(field: &str, reason: &str) -> ApiError {
    ApiError::validation(vec![FieldError {
        field: field.into(),
        message: reason.into(),
        code: None,
    }])
}
fn decode<T: DeserializeOwned>(v: Value, required: &[&str]) -> Result<T, ApiError> {
    for field in required {
        if v.get(*field).is_none_or(Value::is_null) {
            return Err(invalid(field, "Required field"));
        }
    }
    if let Some(obj) = v.as_object() {
        for (field, value) in obj {
            if value.is_null() {
                continue;
            }
            match field.as_str() {
                "item_id" | "owner_id" | "occurrence_id" | "after_id" | "before_id" => {
                    field_decode::<Uuid>(field, value.clone())
                        .map_err(|_| reference_error(field, "invalid-reference"))?;
                }
                "viewers" => {
                    field_decode::<Vec<Uuid>>(field, value.clone())
                        .map_err(|_| reference_error(field, "invalid-reference"))?;
                }
                "qty" => {
                    field_decode::<f64>(field, value.clone())?;
                }
                "mode" => {
                    field_decode::<ListMode>(field, value.clone())?;
                }
                "visibility" => {
                    field_decode::<ListVisibility>(field, value.clone())?;
                }
                "name" | "label" | "unit" | "url" | "notes" => {
                    field_decode::<String>(field, value.clone())?;
                }
                _ => (),
            }
        }
    }
    serde_json::from_value(v).map_err(|e| {
        let message = e.to_string();
        let field = message.split('`').nth(1).unwrap_or("body");
        invalid(field, "Invalid fields")
    })
}
async fn row(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    deleted: bool,
) -> Result<ListRow, ApiError> {
    sqlx::query_as("SELECT * FROM porchlight.lists WHERE id=$1 AND household_id=$2 AND ($3 OR deleted_at IS NULL)")
        .bind(id)
        .bind(c.household_id())
        .bind(deleted)
        .fetch_optional(conn).await?.ok_or_else(|| ApiError::new(Problem::NotFound))
}
async fn entry(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    archived: bool,
) -> Result<EntryRow, ApiError> {
    sqlx::query_as("SELECT * FROM porchlight.list_entries WHERE id=$1 AND household_id=$2 AND ($3 OR archived_at IS NULL)")
        .bind(id)
        .bind(c.household_id())
        .bind(archived)
        .fetch_optional(conn).await?.ok_or_else(|| ApiError::new(Problem::NotFound))
}
async fn viewers(conn: &mut PgConnection, id: Uuid) -> Result<Vec<Uuid>, ApiError> {
    Ok(sqlx::query_scalar(
        "SELECT member_id FROM porchlight.list_viewers WHERE list_id=$1 ORDER BY member_id",
    )
    .bind(id)
    .fetch_all(conn)
    .await?)
}
async fn capabilities(
    conn: &mut PgConnection,
    c: &Caller,
    r: &ListRow,
) -> Result<Vec<ListCapability>, ApiError> {
    if let Some(item) = r.item_id {
        let caps = crate::items::resource_capabilities(conn, c, item).await?;
        if !caps.contains(&Capability::View) {
            return Err(ApiError::new(Problem::NotFound));
        }
        return Ok(ListCapability::ALL
            .into_iter()
            .filter(|cap| caps.contains(&cap.item_capability()))
            .filter(|cap| {
                !matches!(
                    cap,
                    ListCapability::ChangeVisibility | ListCapability::Reassign
                )
            })
            .collect());
    }
    let caller = match c {
        Caller::Agent { .. } => CallerKind::Agent,
        Caller::Member { role, .. } => CallerKind::AppMember(
            role.parse::<Role>()
                .map_err(|_| ApiError::new(Problem::Forbidden))?,
        ),
    };
    let vis = match r.visibility {
        ListVisibility::Household => Visibility::Household,
        ListVisibility::Private => Visibility::Private,
        ListVisibility::Selected => Visibility::Selected,
    };
    let context = access::standalone_list_context(
        caller,
        vis,
        r.owner_id == c.member_id(),
        viewers(conn, r.id).await?.contains(&c.member_id()),
    );
    if !access::standalone_list_can(&context, ListCapability::View) {
        return Err(ApiError::new(Problem::NotFound));
    }
    Ok(ListCapability::ALL
        .into_iter()
        .filter(|cap| access::standalone_list_can(&context, *cap))
        .collect())
}

async fn authorize(
    conn: &mut PgConnection,
    c: &Caller,
    r: &ListRow,
    cap: ListCapability,
) -> Result<(), ApiError> {
    if !capabilities(conn, c, r).await?.contains(&cap) {
        return Err(ApiError::new(Problem::Forbidden));
    }
    Ok(())
}
async fn occurrence(
    conn: &mut PgConnection,
    r: &ListRow,
    id: Option<Uuid>,
    required: bool,
) -> Result<(), ApiError> {
    match r.mode {
        ListMode::Persistent if id.is_some() => {
            return Err(reference_error("occurrence_id", "invalid-reference"));
        }
        ListMode::PerOccurrence if required && id.is_none() => {
            return Err(reference_error("occurrence_id", "invalid-reference"));
        }
        _ => (),
    }
    if let Some(id) = id {
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1
          FROM porchlight.item_occurrences
          WHERE id=$1
          AND household_id=$3
          AND item_id IN (WITH RECURSIVE descendants AS (SELECT id
          FROM porchlight.items
          WHERE id=$2 AND household_id=$3 UNION ALL SELECT i.id
          FROM porchlight.items i JOIN descendants d ON i.split_from_item_id=d.id AND i.household_id=$3) CYCLE id SET is_cycle USING path SELECT id
          FROM descendants WHERE NOT is_cycle))",
        )
        .bind(id)
        .bind(r.item_id)
        .bind(r.household_id)
        .fetch_one(conn)
        .await?;
        if !valid {
            return Err(reference_error("occurrence_id", "invalid-reference"));
        }
    }
    Ok(())
}
async fn entry_response(
    conn: &mut PgConnection,
    r: EntryRow,
    occ: Option<Uuid>,
) -> Result<EntryResponse, ApiError> {
    let check: Option<(Uuid,DateTime<Utc>)> = sqlx::query_as("SELECT checked_by,checked_at FROM porchlight.list_entry_checks WHERE entry_id=$1 AND occurrence_id IS NOT DISTINCT FROM $2")
        .bind(r.id)
        .bind(occ)
        .fetch_optional(conn).await?;
    Ok(EntryResponse {
        fields: r,
        checked_by: check.map(|x| x.0),
        checked_at: check.map(|x| x.1),
        occurrence_id: occ,
    })
}
async fn orphan_cleanup_allowed(
    conn: &mut PgConnection,
    c: &Caller,
    r: &ListRow,
) -> Result<bool, ApiError> {
    if r.item_id.is_none() || (r.owner_id != c.member_id() && c.require_admin().is_err()) {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.items WHERE id=$1 AND household_id=$2 AND deleted_at IS NOT NULL)")
        .bind(r.item_id).bind(r.household_id).fetch_one(conn).await?)
}
pub async fn response(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    occ: Option<Uuid>,
    deleted: bool,
) -> Result<ListResponse, ApiError> {
    response_with_orphans(conn, c, id, occ, deleted, false).await
}
async fn response_with_orphans(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    occ: Option<Uuid>,
    deleted: bool,
    include_orphaned: bool,
) -> Result<ListResponse, ApiError> {
    let r = row(conn, c, id, deleted).await?;
    let caps = if include_orphaned && orphan_cleanup_allowed(conn, c, &r).await? {
        // Cleanup authority is not content authority. Reuse the item ACL while
        // ignoring only its tombstone, before loading entries, checks or viewers.
        match crate::items::load(conn, c, r.item_id.expect("orphan parent"), true).await {
            Ok(_) => (),
            Err(e) if e.problem() == Problem::NotFound => return Ok(ListResponse::cleanup(r)),
            Err(e) => return Err(e),
        }
        vec![ListCapability::View, ListCapability::Delete]
    } else {
        capabilities(conn, c, &r).await?
    };
    occurrence(conn, &r, occ, false).await?;
    let vs = viewers(conn, id).await?;
    let rows: Vec<EntryRow> = sqlx::query_as("SELECT * FROM porchlight.list_entries WHERE list_id=$1 AND archived_at IS NULL ORDER BY position COLLATE \"C\",id")
        .bind(id)
        .fetch_all(&mut *conn).await?;
    let checks: Vec<(Uuid, Uuid, DateTime<Utc>)> = sqlx::query_as(
        "SELECT c.entry_id,c.checked_by,c.checked_at
          FROM porchlight.list_entry_checks c JOIN porchlight.list_entries e ON e.id=c.entry_id
          WHERE e.list_id=$1
          AND c.occurrence_id IS NOT DISTINCT
          FROM $2",
    )
    .bind(id)
    .bind(occ)
    .fetch_all(&mut *conn)
    .await?;
    let checks: std::collections::HashMap<_, _> = checks
        .into_iter()
        .map(|(entry, member, at)| (entry, (member, at)))
        .collect();
    let mut entries = vec![];
    let mut got_it = vec![];
    for e in rows {
        let check = checks.get(&e.id).copied();
        let e = EntryResponse {
            fields: e,
            checked_by: check.map(|c| c.0),
            checked_at: check.map(|c| c.1),
            occurrence_id: occ,
        };
        if matches!(r.mode, ListMode::Persistent) && e.checked_at.is_some() {
            got_it.push(e);
        } else {
            entries.push(e);
        }
    }
    Ok(ListResponse {
        fields: ListFields {
            id: r.id,
            item_id: r.item_id,
            owner_id: r.owner_id,
            mode: r.mode,
            version: r.version,
            redacted: None,
            household_id: Some(r.household_id),
            name: Some(r.name),
            visibility: Some(r.visibility),
            deleted_at: Some(r.deleted_at),
            created_at: Some(r.created_at),
            updated_at: Some(r.updated_at),
        },
        viewers: vs,
        capabilities: caps.into_iter().map(|x| x.as_str().into()).collect(),
        entries,
        got_it,
        occurrence_id: occ,
    })
}
pub async fn collection(
    s: &AppState,
    conn: &mut PgConnection,
    c: &Caller,
    q: ListsQuery,
) -> Result<ListsPage, ApiError> {
    let limit = crate::pagination::PageParams {
        cursor: q.cursor.clone(),
        limit: q.limit,
    }
    .limit()?;
    let context = format!(
        "lists:{}:{}:{}:{:?}:{}",
        c.household_id(),
        c.member_id(),
        c.sees_all(),
        q.item_id,
        q.include_orphaned
    );
    let cursor = q
        .cursor
        .as_ref()
        .map(|x| s.idem.cursor.decode::<Uuid>(&context, x))
        .transpose()?;
    // pl_list_can_read delegates attached visibility to S4b; Rust decorates the same authoritative rule.
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id
          FROM porchlight.lists
          WHERE household_id=$1
          AND deleted_at IS NULL
          AND ((item_id IS NULL OR EXISTS(SELECT 1 FROM porchlight.items p
          WHERE p.id=item_id AND p.deleted_at IS NULL)) AND ($4 OR porchlight.pl_list_can_read(id,$5))
          OR ($7 AND (owner_id=$5 OR $8) AND EXISTS(SELECT 1 FROM porchlight.items p
          WHERE p.id=item_id AND p.deleted_at IS NOT NULL)))
          AND ($2::uuid IS NULL OR id>$2)
          AND ($3::uuid IS NULL OR item_id=$3 OR (mode='per_occurrence'
          AND item_id IN (WITH RECURSIVE ancestors AS (SELECT id,split_from_item_id
          FROM porchlight.items
          WHERE id=$3 AND household_id=$1 UNION ALL SELECT p.id,p.split_from_item_id
          FROM porchlight.items p JOIN ancestors a ON p.id=a.split_from_item_id AND p.household_id=$1) CYCLE id SET is_cycle USING path SELECT id
          FROM ancestors WHERE NOT is_cycle)))
          ORDER BY id LIMIT $6",
    )
    .bind(c.household_id())
    .bind(cursor)
    .bind(q.item_id)
    .bind(c.sees_all())
    .bind(c.member_id())
    .bind((limit + 1) as i64)
    .bind(q.include_orphaned)
    .bind(c.require_admin().is_ok())
    .fetch_all(&mut *conn)
    .await?;
    let page = s.idem.cursor.page(&context, ids, limit, |id| *id)?;
    let mut items = vec![];
    for id in page.items {
        match response_with_orphans(conn, c, id, None, false, q.include_orphaned).await {
            Ok(r) => items.push(r),
            Err(e) if e.problem() == Problem::NotFound => (),
            Err(e) => return Err(e),
        }
    }
    Ok(ListsPage {
        items,
        next_cursor: page.next_cursor,
    })
}
/// Contribute on the shared read-only repeatable-read snapshot connection, never a separate connection.
pub async fn snapshot_contribution(
    conn: &mut PgConnection,
    c: &Caller,
) -> Result<Vec<ListResponse>, ApiError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id
          FROM porchlight.lists
          WHERE household_id=$1
          AND deleted_at IS NULL
          AND (item_id IS NULL OR EXISTS(SELECT 1
          FROM porchlight.items p
          WHERE p.id=item_id
          AND p.deleted_at IS NULL))
          AND ($2 OR porchlight.pl_list_can_read(id,$3))
          ORDER BY id",
    )
    .bind(c.household_id())
    .bind(c.sees_all())
    .bind(c.member_id())
    .fetch_all(&mut *conn)
    .await?;
    let mut result = vec![];
    for id in ids {
        match response(conn, c, id, None, false).await {
            Ok(r) => result.push(r),
            Err(e) if e.problem() == Problem::NotFound => (),
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}
/// Source projection for the shared search UNION. Binds household/member/agent/query as $1..$4.
/// The owner applies the source/id keyset and limit to the combined sources before decoration.
pub const SEARCH_SOURCE: &str = "SELECT 'list'::text AS source,l.id,NULL::jsonb AS value
    FROM porchlight.lists l WHERE l.household_id=$1 AND l.deleted_at IS NULL
    AND (l.item_id IS NULL OR EXISTS(SELECT 1 FROM porchlight.items p WHERE p.id=l.item_id AND p.deleted_at IS NULL))
    AND ($3 OR porchlight.pl_list_can_read(l.id,$2))
    AND (l.search_document @@ plainto_tsquery('simple',$4) OR EXISTS(SELECT 1 FROM porchlight.list_entries e
    WHERE e.list_id=l.id AND e.archived_at IS NULL AND e.search_document @@ plainto_tsquery('simple',$4)))";
/// Test support for ACL-filtered IDs; runtime HTTP search uses `SEARCH_SOURCE`
/// in the shared paginated search, not this unpaginated helper.
#[doc(hidden)]
pub async fn search_contribution(
    conn: &mut PgConnection,
    c: &Caller,
    q: &str,
) -> Result<Vec<Uuid>, ApiError> {
    let sql = format!("SELECT id FROM ({SEARCH_SOURCE}) sources ORDER BY id");
    let ids: Vec<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(c.member_id())
        .bind(c.sees_all())
        .bind(q)
        .fetch_all(&mut *conn)
        .await?;
    let mut result = vec![];
    for id in ids {
        let r = row(conn, c, id, false).await?;
        match capabilities(conn, c, &r).await {
            Ok(_) => result.push(id),
            Err(e) if e.problem() == Problem::NotFound => (),
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}
// List/check mutations intentionally emit only their transactional outbox tickle.
// S7a registers no business kind in R53's notification kind data registry.
async fn members(
    conn: &mut PgConnection,
    c: &Caller,
    ids: &[Uuid],
    field: &str,
) -> Result<(), ApiError> {
    let count: i64=sqlx::query_scalar("SELECT count(*) FROM porchlight.household_members WHERE household_id=$1 AND id=ANY($2) AND deleted_at IS NULL AND role<>'dependent'")
        .bind(c.household_id())
        .bind(ids)
        .fetch_one(conn).await?;
    let unique: std::collections::BTreeSet<_> = ids.iter().collect();
    if count as usize != unique.len() {
        return Err(reference_error(field, "invalid-reference"));
    }
    Ok(())
}
fn validate_entry(e: &EntryRow) -> Result<(), ApiError> {
    if e.label.trim().is_empty() || e.label.chars().count() > 1000 {
        return Err(invalid("label", "Expected 1–1000 characters"));
    }
    if e.qty.is_some_and(|q| !q.is_finite() || q <= 0.0) {
        return Err(invalid("qty", "Expected a positive finite quantity"));
    }
    if e.unit.chars().count() > 100 || e.notes.chars().count() > 10000 {
        return Err(invalid("body", "Entry text too long"));
    }
    if let Some(url) = &e.url
        && (url.len() > 2048
            || !(url.starts_with("https://") || url.starts_with("http://"))
            || url.chars().any(char::is_whitespace)
            || url.split("://").nth(1).is_none_or(str::is_empty))
    {
        return Err(invalid("url", "Expected an HTTP product link"));
    }
    Ok(())
}
async fn position(
    conn: &mut PgConnection,
    list: Uuid,
    mover: Uuid,
    after: Option<Uuid>,
    before: Option<Uuid>,
) -> Result<String, ApiError> {
    let rows: Vec<(Uuid,String)>=sqlx::query_as("SELECT id,position FROM porchlight.list_entries WHERE list_id=$1 AND archived_at IS NULL AND id<>$2 ORDER BY position COLLATE \"C\",id")
        .bind(list)
        .bind(mover)
        .fetch_all(&mut *conn).await?;
    let locate = |id, field| {
        rows.iter()
            .position(|x| x.0 == id)
            .ok_or_else(|| reference_error(field, "invalid-reference"))
    };
    let a = after.map(|id| locate(id, "after_id")).transpose()?;
    let b = before.map(|id| locate(id, "before_id")).transpose()?;
    if matches!((a,b),(Some(a),Some(b)) if a>=b) {
        return Err(reference_error("before_id", "stale-neighbour"));
    }
    let at = if let Some(a) = a {
        a + 1
    } else if let Some(b) = b {
        b
    } else {
        rows.len()
    };
    let lower = at.checked_sub(1).map(|i| rows[i].1.as_str());
    let upper = rows.get(at).map(|x| x.1.as_str());
    if let Ok(key) = ordering::key_between(lower, upper) {
        return Ok(key);
    }
    let (start, end, keys) =
        ordering::insertion_run(&rows.iter().map(|r| r.1.clone()).collect::<Vec<_>>(), at)
            .map_err(|_| reference_error("before_id", "stale-neighbour"))?;
    // Sorted NOWAIT locks avoid waiting behind raw child-first writes (including a move's already locked mover).
    let mut ids = rows[start..end].iter().map(|r| r.0).collect::<Vec<_>>();
    ids.sort();
    sqlx::query("SELECT id FROM porchlight.list_entries WHERE id=ANY($1) ORDER BY id FOR NO KEY UPDATE NOWAIT")
        .bind(ids)
        .fetch_all(&mut *conn).await?;
    for (i, (id, old)) in rows[start..end].iter().enumerate() {
        let key = &keys[if start + i < at { i } else { i + 1 }];
        if old != key {
            sqlx::query("UPDATE porchlight.list_entries SET position=$2 WHERE id=$1")
                .bind(id)
                .bind(key)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(keys[at - start].clone())
}
pub async fn mutate(
    c: Caller,
    tx: &mut IdemTx,
    body: Value,
    op: Operation,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let id = tx.context.target_id.unwrap_or(Uuid::nil());
    let headers = tx.context.if_match.clone();
    let occurrence_id = body
        .get("occurrence_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok());
    let mut domain = tx.tx.begin().await?;
    let result = execute(&mut domain, &c, id, &headers, body, op, now).await;
    match result {
        Ok(r) => {
            domain.commit().await?;
            Ok(r)
        }
        Err(e) => {
            domain.rollback().await?;
            if e.problem() == Problem::VersionConflict {
                let current = if matches!(
                    op,
                    Operation::PatchEntry
                        | Operation::DeleteEntry
                        | Operation::Check
                        | Operation::Uncheck
                ) {
                    let r = entry(&mut tx.tx, &c, id, false).await?;
                    let l = row(&mut tx.tx, &c, r.list_id, false).await?;
                    authorize(&mut tx.tx, &c, &l, ListCapability::View).await?;
                    serde_json::to_value(entry_response(&mut tx.tx, r, occurrence_id).await?)
                } else {
                    serde_json::to_value(
                        response_with_orphans(
                            &mut tx.tx,
                            &c,
                            id,
                            None,
                            false,
                            matches!(op, Operation::Delete),
                        )
                        .await?,
                    )
                }
                .map_err(|_| ApiError::internal("list_encode"))?;
                Err(e.extension("current", current))
            } else {
                Err(e)
            }
        }
    }
}
async fn execute(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    headers: &HeaderMap,
    body: Value,
    op: Operation,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    if let Caller::Agent { scopes, .. } = c
        && !crate::extract::pat_allows_scope(scopes, "write")
    {
        return Err(ApiError::new(Problem::Forbidden));
    }
    if op == Operation::Create {
        let b: CreateList = decode(body, &["name"])?;
        let owner = b.owner_id.unwrap_or(c.member_id());
        members(conn, c, &[owner], "owner_id").await?;
        if !c.sees_all() && owner != c.member_id() && c.require_admin().is_err() {
            return Err(ApiError::new(Problem::Forbidden));
        }
        if b.name.trim().is_empty() || b.name.chars().count() > 300 {
            return Err(invalid("name", "Expected 1–300 characters"));
        }
        let mode = b.mode.unwrap_or(ListMode::Persistent);
        let vis = b.visibility.unwrap_or(ListVisibility::Household);
        let vs = b.viewers.unwrap_or_default();
        members(conn, c, &vs, "viewers").await?;
        if let Some(item) = b.item_id {
            sqlx::query(
                "SELECT id FROM porchlight.items WHERE id=$1 AND household_id=$2 FOR NO KEY UPDATE",
            )
            .bind(item)
            .bind(c.household_id())
            .fetch_optional(&mut *conn)
            .await?;
            if !crate::items::resource_capabilities(conn, c, item)
                .await
                .map_err(|e| {
                    if e.problem() == Problem::NotFound {
                        reference_error("item_id", "invalid-reference")
                    } else {
                        e
                    }
                })?
                .contains(&Capability::Edit)
            {
                return Err(ApiError::new(Problem::Forbidden));
            }
            if matches!(mode, ListMode::PerOccurrence) {
                let recurring:bool=sqlx::query_scalar("SELECT rrule IS NOT NULL OR coalesce(recurrence_mode='completion',false) FROM porchlight.items WHERE id=$1")
        .bind(item)
        .fetch_one(&mut *conn).await?;
                if !recurring {
                    return Err(invalid(
                        "mode",
                        "Per-occurrence lists require a recurring item",
                    ));
                }
            }
            if !vs.is_empty() || !matches!(vis, ListVisibility::Household) {
                return Err(invalid(
                    "visibility",
                    "Attached list inherits item visibility",
                ));
            }
        } else if matches!(mode, ListMode::PerOccurrence) {
            return Err(invalid("mode", "Per-occurrence lists require an item"));
        }
        let id: Uuid=sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,item_id,owner_id,name,mode,visibility) VALUES($1,$2,$3,$4,$5,$6) RETURNING id")
        .bind(c.household_id())
        .bind(b.item_id)
        .bind(owner)
        .bind(b.name)
        .bind(if matches!(mode,ListMode::Persistent){"persistent"}else{"per_occurrence"})
        .bind(vis_text(&vis))
        .fetch_one(&mut *conn).await?;
        replace_viewers(conn, c, id, &vs).await?;
        return Ok((
            StatusCode::CREATED,
            Json(response(conn, c, id, None, false).await?),
        )
            .into_response());
    }
    let entry_op = matches!(
        op,
        Operation::PatchEntry | Operation::DeleteEntry | Operation::Check | Operation::Uncheck
    );
    let list_id = if entry_op {
        entry(conn, c, id, false).await?.list_id
    } else {
        id
    };
    let initial = row(conn, c, list_id, false).await?;
    // No WriteScope: lists never modify items, occurrences, agenda or derivation. Parent item serializes ACL changes.
    if let Some(item) = initial.item_id {
        sqlx::query("SELECT id FROM porchlight.items WHERE id=$1 FOR NO KEY UPDATE")
            .bind(item)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("SELECT id FROM porchlight.lists WHERE id=$1 FOR NO KEY UPDATE")
        .bind(list_id)
        .execute(&mut *conn)
        .await?;
    let l = row(conn, c, list_id, false).await?;
    let cap = match op {
        Operation::Check | Operation::Uncheck => ListCapability::Check,
        Operation::Delete => ListCapability::Delete,
        Operation::Viewers => ListCapability::ChangeVisibility,
        Operation::Patch => ListCapability::Edit,
        _ => ListCapability::Manage,
    };
    let orphan_delete = op == Operation::Delete && orphan_cleanup_allowed(conn, c, &l).await?;
    if !orphan_delete {
        authorize(conn, c, &l, cap).await?;
    }
    let mut e = if entry_op {
        sqlx::query("SELECT id FROM porchlight.list_entries WHERE id=$1 FOR NO KEY UPDATE")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        Some(entry(conn, c, id, false).await?)
    } else {
        None
    };
    if op != Operation::AddEntry {
        let expected = crate::protocol::if_match(headers)?;
        if expected != e.as_ref().map_or(l.version, |e| e.version) {
            return Err(ApiError::new(Problem::VersionConflict));
        }
    }
    match op {
        Operation::Patch => {
            let b: PatchList = decode(body, &[])?;
            if b.name
                .as_ref()
                .is_some_and(|n| n.trim().is_empty() || n.chars().count() > 300)
            {
                return Err(invalid("name", "Expected 1–300 characters"));
            }
            if l.item_id.is_some() && (b.visibility.is_some() || b.owner_id.is_some()) {
                return Err(invalid(
                    "visibility",
                    "Attached list inherits item relationships",
                ));
            }
            if b.visibility.is_some() {
                authorize(conn, c, &l, ListCapability::ChangeVisibility).await?;
            }
            if let Some(owner) = b.owner_id {
                authorize(conn, c, &l, ListCapability::Reassign).await?;
                members(conn, c, &[owner], "owner_id").await?;
            }
            if b.name.is_some() || b.owner_id.is_some() || b.visibility.is_some() {
                sqlx::query("UPDATE porchlight.lists
          SET name=coalesce($2,name),owner_id=coalesce($3,owner_id),visibility=coalesce($4,visibility)
          WHERE id=$1
          AND (name,owner_id,visibility) IS DISTINCT
          FROM (coalesce($2,name),coalesce($3,owner_id),coalesce($4,visibility))")
        .bind(id)
        .bind(b.name)
        .bind(b.owner_id)
        .bind(b.visibility.as_ref().map(vis_text))
        .execute(&mut *conn).await?;
            }
            match response(conn, c, id, None, false).await {
                Ok(r) => Ok(Json(r).into_response()),
                Err(e) if e.problem() == Problem::NotFound => {
                    Ok(StatusCode::NO_CONTENT.into_response())
                }
                Err(e) => Err(e),
            }
        }
        Operation::Delete => {
            sqlx::query("UPDATE porchlight.lists SET deleted_at=$2 WHERE id=$1")
                .bind(id)
                .bind(now)
                .execute(&mut *conn)
                .await?;
            if orphan_delete {
                Ok(StatusCode::NO_CONTENT.into_response())
            } else {
                Ok(Json(response(conn, c, id, None, true).await?).into_response())
            }
        }
        Operation::Viewers => {
            let b: ViewersRequest = decode(body, &["viewers"])?;
            members(conn, c, &b.viewers, "viewers").await?;
            replace_viewers(conn, c, id, &b.viewers).await?;
            match response(conn, c, id, None, false).await {
                Ok(r) => Ok(Json(r).into_response()),
                Err(e) if e.problem() == Problem::NotFound => {
                    Ok(StatusCode::NO_CONTENT.into_response())
                }
                Err(e) => Err(e),
            }
        }
        Operation::Clear => {
            let b: CheckRequest = decode(body, &[])?;
            occurrence(conn, &l, b.occurrence_id, false).await?;
            if !matches!(l.mode, ListMode::Persistent) {
                return Err(invalid("mode", "Only persistent Got it can be cleared"));
            }
            sqlx::query("SELECT id FROM porchlight.list_entries WHERE list_id=$1 AND archived_at IS NULL ORDER BY id FOR NO KEY UPDATE")
        .bind(id)
        .fetch_all(&mut *conn).await?;
            let changed = sqlx::query(
                "UPDATE porchlight.list_entries e
          SET archived_at=$2
          WHERE list_id=$1
          AND archived_at IS NULL
          AND EXISTS(SELECT 1
          FROM porchlight.list_entry_checks c
          WHERE c.entry_id=e.id
          AND c.occurrence_id IS NULL)",
            )
            .bind(id)
            .bind(now)
            .execute(&mut *conn)
            .await?;
            if changed.rows_affected() > 0 {
                sqlx::query("UPDATE porchlight.lists SET updated_at=updated_at WHERE id=$1")
                    .bind(id)
                    .execute(&mut *conn)
                    .await?;
            }
            Ok(Json(response(conn, c, id, None, false).await?).into_response())
        }
        Operation::Check | Operation::Uncheck => {
            let b: CheckRequest = decode(body, &[])?;
            occurrence(conn, &l, b.occurrence_id, true).await?;
            if op == Operation::Check {
                sqlx::query("INSERT INTO porchlight.list_entry_checks(household_id,entry_id,occurrence_id,checked_by,checked_at)
          VALUES($1,$2,$3,$4,$5) ON CONFLICT(entry_id,occurrence_id) DO NOTHING")
        .bind(c.household_id())
        .bind(id)
        .bind(b.occurrence_id)
        .bind(c.member_id())
        .bind(now)
        .execute(&mut *conn).await?;
            } else {
                sqlx::query("DELETE FROM porchlight.list_entry_checks WHERE entry_id=$1 AND occurrence_id IS NOT DISTINCT FROM $2")
        .bind(id)
        .bind(b.occurrence_id)
        .execute(&mut *conn).await?;
            }
            let r = entry(conn, c, id, false).await?;
            Ok(Json(entry_response(conn, r, b.occurrence_id).await?).into_response())
        }
        Operation::DeleteEntry => {
            sqlx::query("UPDATE porchlight.list_entries SET archived_at=$2 WHERE id=$1")
                .bind(id)
                .bind(now)
                .execute(&mut *conn)
                .await?;
            let r = entry(conn, c, id, true).await?;
            Ok(Json(entry_response(conn, r, None).await?).into_response())
        }
        Operation::AddEntry | Operation::PatchEntry => {
            let b: EntryWrite = decode(body.clone(), &[])?;
            let mut r = e.take().unwrap_or(EntryRow {
                id: Uuid::now_v7(),
                household_id: c.household_id(),
                list_id,
                label: String::new(),
                qty: None,
                unit: String::new(),
                url: None,
                notes: String::new(),
                position: ordering::first_key(),
                archived_at: None,
                version: 1,
                created_at: now,
                updated_at: now,
            });
            if let Some(x) = b.label {
                r.label = x;
            }
            if body.get("qty").is_some() {
                r.qty = b.qty;
            }
            if let Some(x) = b.unit {
                r.unit = x;
            }
            if body.get("url").is_some() {
                r.url = b.url;
            }
            if let Some(x) = b.notes {
                r.notes = x;
            }
            validate_entry(&r)?;
            if op == Operation::AddEntry
                || body.get("after_id").is_some()
                || body.get("before_id").is_some()
            {
                r.position = position(conn, list_id, r.id, b.after_id, b.before_id).await?;
            }
            if op == Operation::AddEntry {
                sqlx::query("INSERT INTO porchlight.list_entries(id,household_id,list_id,label,qty,unit,url,notes,position) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(r.id)
        .bind(r.household_id)
        .bind(r.list_id)
        .bind(r.label)
        .bind(r.qty)
        .bind(r.unit)
        .bind(r.url)
        .bind(r.notes)
        .bind(r.position)
        .execute(&mut *conn).await?;
            } else if body.as_object().is_some_and(|b| !b.is_empty()) {
                sqlx::query(
                    "UPDATE porchlight.list_entries
          SET label=$2,qty=$3,unit=$4,url=$5,notes=$6,position=$7
          WHERE id=$1
          AND (label,qty,unit,url,notes,position) IS DISTINCT
          FROM ($2,$3,$4,$5,$6,$7)",
                )
                .bind(r.id)
                .bind(r.label)
                .bind(r.qty)
                .bind(r.unit)
                .bind(r.url)
                .bind(r.notes)
                .bind(r.position)
                .execute(&mut *conn)
                .await?;
            }
            let r = entry(conn, c, r.id, false).await?;
            Ok((
                if op == Operation::AddEntry {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(entry_response(conn, r, None).await?),
            )
                .into_response())
        }
        Operation::Create => unreachable!(),
    }
}
fn vis_text(vis: &ListVisibility) -> &'static str {
    match vis {
        ListVisibility::Household => "household",
        ListVisibility::Private => "private",
        ListVisibility::Selected => "selected",
    }
}
async fn replace_viewers(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    vs: &[Uuid],
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM porchlight.list_viewers WHERE list_id=$1 AND NOT(member_id=ANY($2))")
        .bind(id)
        .bind(vs)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO porchlight.list_viewers(household_id,list_id,member_id) SELECT $1,$2,x FROM unnest($3::uuid[]) x ON CONFLICT DO NOTHING")
        .bind(c.household_id())
        .bind(id)
        .bind(vs)
        .execute(conn).await?;
    Ok(())
}
