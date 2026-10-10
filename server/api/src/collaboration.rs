//! S7b comment service; every mutation's SQL runs on the supplied IdemTx.
use crate::{
    error::{ApiError, FieldError, Problem},
    extract::Caller,
    idempotency::IdemTx,
    items,
    routes::reads::VISIBLE,
    state::AppState,
};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use porchlight_core::access::Capability;
use porchlight_db::items::{ScopeRequest, WriteScope};
use serde::{Deserialize, Serialize};
use sqlx::{Acquire, PgConnection, Postgres, Transaction};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Comment {
    pub id: Uuid,
    pub item_id: Uuid,
    pub author_id: Option<Uuid>,
    pub author_token_id: Option<Uuid>,
    pub body: String,
    pub mentions: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub version: i32,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CommentBody {
    pub body: String,
}
macro_rules! comment_columns {
    () => {
        "id,item_id,author_id,author_token_id,body,mentions,created_at,edited_at,deleted_at,version"
    };
}
pub(crate) use comment_columns;
pub fn invalid(field: &str, message: &str) -> ApiError {
    ApiError::validation(vec![FieldError {
        field: field.into(),
        message: message.into(),
        code: None,
    }])
}
pub fn target(tx: &IdemTx) -> Result<Uuid, ApiError> {
    tx.context
        .target_id
        .ok_or_else(|| ApiError::new(Problem::InvalidRequest))
}
pub fn identities(c: &Caller) -> (Option<Uuid>, Option<Uuid>) {
    match c {
        Caller::Member { member_id, .. } => (Some(*member_id), None),
        Caller::Agent { token_id, .. } => (None, Some(*token_id)),
    }
}
pub fn owns(c: &Caller, member: Option<Uuid>, token: Option<Uuid>) -> bool {
    identities(c) == (member, token)
}
pub fn super_admin(c: &Caller) -> bool {
    match c {
        Caller::Member { role, .. } => role == "super_admin",
        Caller::Agent { .. } => true,
    }
}
pub async fn comment(conn: &mut PgConnection, c: &Caller, id: Uuid) -> Result<Comment, ApiError> {
    sqlx::query_as(LOAD_COMMENT)
        .bind(c.household_id())
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| ApiError::new(Problem::NotFound))
}
/// One retained scope, even for collaboration-only writes. Items serialize ACL changes and child edits.
pub async fn scope(
    tx: &mut Transaction<'_, Postgres>,
    c: &Caller,
    item: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let scope = WriteScope::begin_at(
        tx,
        c.household_id(),
        ScopeRequest {
            items: vec![item],
            members: vec![c.member_id()],
            ..Default::default()
        },
        now,
    )
    .await
    .map_err(items::domain_error)?;
    scope.finish(now).await.map_err(items::domain_error)?;
    Ok(())
}
pub async fn read_item(conn: &mut PgConnection, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let sql =
        format!("SELECT EXISTS(SELECT 1 FROM porchlight.items i WHERE {VISIBLE} AND i.id=$4)");
    let visible: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(c.member_id())
        .bind(c.sees_all())
        .bind(id)
        .fetch_one(conn)
        .await?;
    if !visible {
        return Err(ApiError::new(Problem::NotFound));
    }
    Ok(())
}

/// Validate a batch of contribution scopes without an N+1 ACL scan.
pub(crate) async fn read_items(
    conn: &mut PgConnection,
    c: &Caller,
    ids: &[Uuid],
) -> Result<(), ApiError> {
    let sql = format!(
        "SELECT NOT EXISTS(SELECT 1 FROM unnest($4::uuid[]) requested(id)
        WHERE NOT EXISTS(SELECT 1 FROM porchlight.items i WHERE i.id=requested.id AND {VISIBLE}))"
    );
    let visible: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(c.member_id())
        .bind(c.sees_all())
        .bind(ids)
        .fetch_one(conn)
        .await?;
    if !visible {
        return Err(ApiError::new(Problem::NotFound));
    }
    Ok(())
}

/// Lenient, case-insensitive longest display-name match at word boundaries.
/// Ambiguous names and unknown @tokens remain plain text (R44); emails are not mentions.
pub fn parse_mentions(body: &str, members: &[(Uuid, String)]) -> Result<Vec<Uuid>, ApiError> {
    if body.trim().is_empty() || body.chars().count() > 10000 || body.contains('\0') {
        return Err(invalid("body", "Use 1–10000 characters of plain text"));
    }
    let mut result = Vec::new();
    let mut consumed = 0;
    for (pos, _) in body.match_indices('@') {
        if pos < consumed
            || body[..pos]
                .chars()
                .next_back()
                .is_some_and(|v| v.is_alphanumeric() || v == '_')
        {
            continue;
        }
        let rest = &body[pos + 1..];
        let quoted = rest.starts_with('"');
        let text = if quoted { &rest[1..] } else { rest };
        // Compare character prefixes, not lowercased byte offsets (Unicode case folds can grow).
        let mut matches: Vec<_> = members
            .iter()
            .filter_map(|(id, name)| {
                let end = text
                    .char_indices()
                    .nth(name.chars().count())
                    .map_or(text.len(), |(i, _)| i);
                let prefix = &text[..end];
                if !name.is_empty()
                    && prefix.to_lowercase() == name.to_lowercase()
                    && if quoted {
                        text[end..].starts_with('"')
                    } else {
                        text[end..]
                            .chars()
                            .next()
                            .is_none_or(|v| !(v.is_alphanumeric() || v == '-' || v == '_'))
                    }
                {
                    Some((*id, end))
                } else {
                    None
                }
            })
            .collect();
        matches.sort_by_key(|(_, end)| std::cmp::Reverse(*end));
        let Some(&(id, end)) = matches.first() else {
            continue;
        };
        if matches.get(1).is_some_and(|(_, other)| *other == end) {
            continue;
        }
        consumed = pos + 1 + end + if quoted { 2 } else { 0 };
        if !result.contains(&id) {
            result.push(id);
        }
    }
    result.sort();
    Ok(result)
}
async fn mentions(conn: &mut PgConnection, c: &Caller, body: &str) -> Result<Vec<Uuid>, ApiError> {
    let members = sqlx::query_as::<_, (Uuid, String)>(LIVE_MEMBER_NAMES)
        .bind(c.household_id())
        .fetch_all(&mut *conn)
        .await?;
    parse_mentions(body, &members)
}
/// Eligible live readers, excluding the actor and already-present mentions.
/// The emitter uses this same SQL source in one batched statement.
pub async fn comment_notification_recipients(
    conn: &mut PgConnection,
    c: &Caller,
    item: Uuid,
    comment: &Comment,
    created: bool,
    old_mentions: &[Uuid],
) -> Result<Vec<(Uuid, &'static str)>, ApiError> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(COMMENT_RECIPIENTS)
        .bind(c.household_id())
        .bind(item)
        .bind(c.member_id())
        .bind(&comment.mentions)
        .bind(old_mentions)
        .bind(created)
        .fetch_all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, kind)| {
            (
                id,
                if kind == "comment" {
                    "comment"
                } else {
                    "mention"
                },
            )
        })
        .collect())
}
/// Immutable comment/mention events, emitted together inside the domain savepoint.
pub async fn emit_comment_notifications_s4b(
    conn: &mut PgConnection,
    c: &Caller,
    item: Uuid,
    comment: &Comment,
    created: bool,
    old_mentions: &[Uuid],
) -> Result<(), ApiError> {
    let sql = format!("SELECT porchlight.pl_emit_notification_event(
        $1, r.id, r.kind, 'comment', $7, jsonb_build_object('item_id',$2,'comment_id',$7),
        $2, NULL, 'comment:' || $7::text || ':' || $8::int::text || ':' || r.id::text || ':' || r.kind)
        FROM ({COMMENT_RECIPIENTS}) r");
    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(item)
        .bind(c.member_id())
        .bind(&comment.mentions)
        .bind(old_mentions)
        .bind(created)
        .bind(comment.id)
        .bind(comment.version)
        .execute(conn)
        .await?;
    Ok(())
}
const COMMENT_RECIPIENTS: &str = "SELECT m.id, k.kind
    FROM porchlight.items i
    JOIN porchlight.household_members m ON m.household_id=i.household_id
    CROSS JOIN LATERAL (VALUES
        ('comment', $6 AND (i.owner_id=m.id OR EXISTS(SELECT 1 FROM porchlight.item_assignees a WHERE a.item_id=i.id AND a.member_id=m.id))),
        ('mention', m.id=ANY($4::uuid[]) AND NOT m.id=ANY($5::uuid[]))
    ) k(kind, eligible)
    WHERE i.household_id=$1 AND i.id=$2 AND m.id<>$3 AND k.eligible
      AND porchlight.pl_item_can_read(i.id,m.id)
    ORDER BY m.id,k.kind";
pub fn editable(comment: &Comment, c: &Caller, now: DateTime<Utc>) -> Result<(), ApiError> {
    if !owns(c, comment.author_id, comment.author_token_id) {
        return Err(ApiError::new(Problem::Forbidden));
    }
    if now > comment.created_at + chrono::Duration::minutes(15) {
        return Err(ApiError::new(Problem::CommentEditExpired));
    }
    Ok(())
}
#[derive(Clone, Copy)]
pub enum CommentOp {
    Create,
    Patch,
    Delete,
}
pub async fn mutate(
    state: &AppState,
    c: Caller,
    tx: &mut IdemTx,
    body: Option<CommentBody>,
    op: CommentOp,
) -> Result<Response, ApiError> {
    let id = target(tx)?;
    let now = (state.item_clock)();
    let headers = tx.context.if_match.clone();
    let mut domain = tx.tx.begin().await?;
    let result = async {
        let prior = if matches!(op, CommentOp::Create) {
            None
        } else {
            Some(comment(&mut domain, &c, id).await?)
        };
        let item = prior.as_ref().map_or(id, |r| r.item_id);
        read_item(&mut domain, &c, item).await?;
        scope(&mut domain, &c, item, now).await?;
        // The edit deadline (and the written timestamps) use the clock read after the item scope
        // wait: queueing on the lock must not extend the inclusive 15-minute window.
        let now = (state.item_clock)();
        let row = items::load(&mut domain, &c, item, false).await?;
        let prior = if prior.is_some() {
            Some(comment(&mut domain, &c, id).await?)
        } else {
            None
        };
        if matches!(op, CommentOp::Create) {
            items::authorize(&mut domain, &c, &row, Capability::Comment).await?;
        }
        if let Some(r) = &prior {
            if matches!(op, CommentOp::Patch) {
                editable(r, &c, now)?;
            } else if !owns(&c, r.author_id, r.author_token_id) && !super_admin(&c) {
                return Err(ApiError::new(Problem::Forbidden));
            }
            if r.version != crate::protocol::if_match(&headers)? {
                return Err(ApiError::new(Problem::VersionConflict).extension(
                    "current",
                    serde_json::to_value(r).map_err(|_| ApiError::internal("comment_encode"))?,
                ));
            }
        }
        if matches!(op, CommentOp::Delete) {
            // Lock the complete union before bulk child updates, in GC/lease hash order.
            sqlx::query(LOCK_COMMENT_BLOBS)
                .bind(c.household_id())
                .bind(id)
                .fetch_all(&mut *domain)
                .await?;
            sqlx::query("UPDATE porchlight.comments SET deleted_at=$2 WHERE id=$1")
                .bind(id)
                .bind(now)
                .execute(&mut *domain)
                .await?;
            // Comment attachments no longer download; soft-delete links so retention starts now.
            sqlx::query(SOFT_DELETE_COMMENT_ATTACHMENTS)
                .bind(id)
                .bind(now)
                .execute(&mut *domain)
                .await?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        let body = body
            .ok_or_else(|| ApiError::new(Problem::InvalidRequest))?
            .body;
        if let Some(r) = &prior
            && r.body == body
        {
            return Ok(Json(r).into_response());
        }
        let mentions = mentions(&mut domain, &c, &body).await?;
        let saved: Comment = if let Some(r) = &prior {
            sqlx::query_as(UPDATE_COMMENT_BODY)
                .bind(r.id)
                .bind(body)
                .bind(mentions)
                .bind(now)
                .fetch_one(&mut *domain)
                .await?
        } else {
            let (member, token) = identities(&c);
            sqlx::query_as(INSERT_COMMENT)
                .bind(c.household_id())
                .bind(item)
                .bind(member)
                .bind(token)
                .bind(body)
                .bind(mentions)
                .fetch_one(&mut *domain)
                .await?
        };
        emit_comment_notifications_s4b(
            &mut domain,
            &c,
            item,
            &saved,
            prior.is_none(),
            prior.as_ref().map_or(&[], |r| &r.mentions),
        )
        .await?;
        Ok((
            if prior.is_none() {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            Json(saved),
        )
            .into_response())
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

/// Transaction-local contribution to the shared repeatable-read snapshot.
pub async fn snapshot_comments_s4b(
    conn: &mut PgConnection,
    c: &Caller,
    item_ids: &[Uuid],
) -> Result<Vec<Comment>, ApiError> {
    read_items(conn, c, item_ids).await?;
    let sql = format!(
        "SELECT c.* FROM porchlight.comments c JOIN porchlight.items i ON i.id=c.item_id
        WHERE {VISIBLE} AND c.deleted_at IS NULL AND i.id=ANY($4) ORDER BY c.id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(c.member_id())
        .bind(c.sees_all())
        .bind(item_ids)
        .fetch_all(conn)
        .await?)
}
/// Shared comment source: expand the VISIBLE placeholder with S4b's canonical ACL.
/// Binds $1..$4 match item and list sources; pagination is applied once by reads::search.
pub const COMMENT_SEARCH_SOURCE: &str = "SELECT 'comment'::text AS source,c.id,
    (to_jsonb(c)-'household_id'-'updated_at'-'search_document') AS value
    FROM porchlight.comments c JOIN porchlight.items i ON i.id=c.item_id
    WHERE {VISIBLE} AND c.deleted_at IS NULL AND c.search_document @@ plainto_tsquery('simple',$4)";

const LOAD_COMMENT: &str = concat!(
    "SELECT ",
    comment_columns!(),
    " FROM porchlight.comments WHERE household_id=$1 AND id=$2 AND deleted_at IS NULL"
);
const LIVE_MEMBER_NAMES: &str = "SELECT id,display_name FROM porchlight.household_members WHERE household_id=$1 AND deleted_at IS NULL ORDER BY id";
const LOCK_COMMENT_BLOBS: &str = "SELECT b.sha256 FROM porchlight.blobs b WHERE b.household_id=$1 AND EXISTS(SELECT 1 FROM porchlight.attachments a WHERE a.comment_id=$2 AND a.deleted_at IS NULL AND (a.blob_sha256=b.sha256 OR a.thumbnail_sha256=b.sha256)) ORDER BY b.sha256 FOR UPDATE";
const SOFT_DELETE_COMMENT_ATTACHMENTS: &str =
    "UPDATE porchlight.attachments SET deleted_at=$2 WHERE comment_id=$1 AND deleted_at IS NULL";
const UPDATE_COMMENT_BODY: &str = concat!(
    "UPDATE porchlight.comments SET body=$2,mentions=$3,edited_at=$4 WHERE id=$1 RETURNING ",
    comment_columns!(),
    ""
);
const INSERT_COMMENT: &str = concat!(
    "INSERT INTO porchlight.comments(household_id,item_id,author_id,author_token_id,body,mentions) VALUES($1,$2,$3,$4,$5,$6) RETURNING ",
    comment_columns!(),
    ""
);
