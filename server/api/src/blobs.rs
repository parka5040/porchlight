//! Safe K6 ingestion, durable content-addressed files, ID-scoped downloads and leased GC.
use crate::{
    collaboration as collab,
    error::{ApiError, Problem},
    extract::{Caller, Mutation},
    idempotency::IdemTx,
    items,
    routes::reads::VISIBLE,
    state::AppState,
};
use axum::{
    Json,
    body::Body,
    extract::{FromRequest, Multipart, Request},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use porchlight_core::{
    access::Capability,
    ingest::{self, IngestError, Ingested, Limits},
};
use serde::{Deserialize, Serialize};
use sqlx::{Acquire, PgConnection};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use utoipa::ToSchema;
use uuid::Uuid;
pub const MAX_UPLOAD: usize = 25 * 1024 * 1024;
mod filename_categories;
mod gc;
mod storage;
mod upload;
pub use gc::{Unlink, gc, gc_with_unlink};
pub use storage::{blob_path, durable_write, sanitize_name};
pub(crate) use upload::admit_ingest;
pub use upload::{Upload, UploadLimits, UploadMutation, UploadStreams, ingest_error, safe_ingest};
#[derive(Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Attachment {
    pub id: Uuid,
    pub item_id: Uuid,
    pub comment_id: Option<Uuid>,
    pub original_name: String,
    pub uploader_id: Option<Uuid>,
    pub uploader_token_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub version: i32,
    pub size: i64,
    pub content_type: String,
    pub has_thumbnail: bool,
}
const ATTACHMENT_SELECT: &str = "SELECT a.id,a.item_id,a.comment_id,a.original_name,a.uploader_id,a.uploader_token_id,a.created_at,a.deleted_at,a.version,b.size,b.content_type,(a.thumbnail_sha256 IS NOT NULL) AS has_thumbnail
    FROM porchlight.attachments a JOIN porchlight.blobs b ON b.household_id=a.household_id AND b.sha256=a.blob_sha256
    JOIN porchlight.items i ON i.id=a.item_id";
const LIVE_ATTACHMENT: &str = "a.deleted_at IS NULL AND (a.comment_id IS NULL OR EXISTS(SELECT 1 FROM porchlight.comments c WHERE c.id=a.comment_id AND c.deleted_at IS NULL))";
pub async fn attachment(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
) -> Result<Attachment, ApiError> {
    let sql = format!("{ATTACHMENT_SELECT} WHERE {VISIBLE} AND {LIVE_ATTACHMENT} AND a.id=$4");
    sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(c.member_id())
        .bind(c.sees_all())
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or_else(|| ApiError::new(Problem::NotFound))
}
pub async fn upload(
    state: &AppState,
    c: Caller,
    tx: &mut IdemTx,
    upload: Upload,
    on_comment: bool,
) -> Result<Response, ApiError> {
    let id = collab::target(tx)?;
    let now = (state.item_clock)();
    // Reject hidden paths before decoder work; SQL stays on IdemTx.
    let item = if on_comment {
        collab::comment(&mut tx.tx, &c, id).await?.item_id
    } else {
        id
    };
    let row = items::load(&mut tx.tx, &c, item, false).await?;
    items::authorize(&mut tx.tx, &c, &row, Capability::Comment).await?;
    if on_comment {
        collab::editable(&collab::comment(&mut tx.tx, &c, id).await?, &c, now)?;
    }
    let (ingested, name) = safe_ingest(state, upload).await?;
    let now = (state.item_clock)();
    let mut domain = tx.tx.begin().await?;
    let result = async {
        collab::scope(&mut domain, &c, item, now).await?;
        let row = items::load(&mut domain, &c, item, false).await?;
        items::authorize(&mut domain, &c, &row, Capability::Comment).await?;
        let comment = if on_comment {
            Some(collab::comment(&mut domain, &c, id).await?)
        } else {
            None
        };
        let mut blobs = vec![(&ingested.sha256, ingested.content_type, &ingested.bytes)];
        if let Some(t) = &ingested.thumbnail {
            blobs.push((&t.sha256, t.content_type, &t.bytes));
        }
        blobs.sort_by_key(|(hash, _, _)| *hash);
        blobs.dedup_by_key(|(hash, _, _)| *hash);
        // Fixed filesystem shards in ascending order, before metadata hashes.
        let shards: std::collections::BTreeSet<_> =
            blobs.iter().map(|(hash, _, _)| &hash[..2]).collect();
        for shard in shards {
            sqlx::query(LOCK_STORAGE_SHARD)
                .bind(shard)
                .fetch_one(&mut *domain)
                .await?;
        }
        // Edit deadline and timestamps use the clock read after the last gate this upload can
        // queue behind (item scope, then shard gates): waiting must not extend the window.
        let now = (state.item_clock)();
        if let Some(comment) = &comment {
            collab::editable(comment, &c, now)?;
        }
        for (hash, mime, bytes) in blobs {
            // Metadata lock precedes FS work, serializing upload and GC. Upsert is idempotent.
            sqlx::query(UPSERT_BLOB_METADATA)
                .bind(c.household_id())
                .bind(hash)
                .bind(bytes.len() as i64)
                .bind(mime)
                .bind(now)
                .execute(&mut *domain)
                .await?;
            let root = state.settings.data_dir.clone();
            let hash = hash.clone();
            let bytes = bytes.clone();
            tokio::task::spawn_blocking(move || durable_write(&root, &hash, &bytes))
                .await
                .map_err(|_| ApiError::internal("blob_task"))??;
        }
        let (member, token) = collab::identities(&c);
        let attachment_id: Uuid = sqlx::query_scalar(INSERT_ATTACHMENT)
            .bind(c.household_id())
            .bind(item)
            .bind(on_comment.then_some(id))
            .bind(&ingested.sha256)
            .bind(ingested.thumbnail.as_ref().map(|t| &t.sha256))
            .bind(name)
            .bind(member)
            .bind(token)
            .fetch_one(&mut *domain)
            .await?;
        if on_comment {
            sqlx::query("UPDATE porchlight.comments SET edited_at=$2 WHERE id=$1")
                .bind(id)
                .bind(now)
                .execute(&mut *domain)
                .await?;
        }
        Ok((
            StatusCode::CREATED,
            Json(attachment(&mut domain, &c, attachment_id).await?),
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
pub async fn download(
    state: &AppState,
    c: Caller,
    id: Uuid,
    thumbnail: bool,
) -> Result<Response, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let r = attachment(&mut conn, &c, id).await?;

    let blob: Option<(String, String, i64)> = sqlx::query_as(DOWNLOAD_BLOB)
        .bind(c.household_id())
        .bind(id)
        .bind(thumbnail)
        .fetch_optional(&mut *conn)
        .await?;
    let (hash, mime, size) = blob.ok_or_else(|| ApiError::new(Problem::NotFound))?;
    let file = tokio::fs::File::open(blob_path(&state.settings.data_dir, &hash)?)
        .await
        .map_err(|_| ApiError::internal("blob_read"))?;
    drop(conn);
    let name = sanitize_name(&r.original_name, &mime);
    let disposition = if mime == "application/pdf" {
        "attachment"
    } else {
        "inline"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            if mime == "text/plain" {
                "text/plain; charset=utf-8"
            } else {
                &mime
            },
        )
        .header(header::CONTENT_LENGTH, size)
        .header(
            header::CONTENT_DISPOSITION,
            storage::content_disposition(disposition, &name),
        )
        .header("x-content-type-options", "nosniff")
        .header("content-security-policy", "sandbox")
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(file)))
        .map_err(|_| ApiError::internal("blob_response"))
}
pub async fn delete(state: &AppState, c: Caller, tx: &mut IdemTx) -> Result<Response, ApiError> {
    let id = collab::target(tx)?;
    let now = (state.item_clock)();
    let headers = tx.context.if_match.clone();
    let mut domain = tx.tx.begin().await?;
    let result = async {
        let r = attachment(&mut domain, &c, id).await?;

        collab::scope(&mut domain, &c, r.item_id, now).await?;
        // A comment attachment's edit deadline is decided with the clock read after the scope wait.
        let now = (state.item_clock)();
        let r = attachment(&mut domain, &c, id).await?;

        if let Some(comment_id) = r.comment_id {
            let comment = collab::comment(&mut domain, &c, comment_id).await?;
            if !collab::super_admin(&c) {
                collab::editable(&comment, &c, now)?;
            }
        } else if !collab::owns(&c, r.uploader_id, r.uploader_token_id) {
            let row = items::load(&mut domain, &c, r.item_id, false).await?;
            items::authorize(&mut domain, &c, &row, Capability::Edit).await?;
        }
        if r.version != crate::protocol::if_match(&headers)? {
            return Err(ApiError::new(Problem::VersionConflict).extension(
                "current",
                serde_json::to_value(r).map_err(|_| ApiError::internal("attachment_encode"))?,
            ));
        }
        sqlx::query("UPDATE porchlight.attachments SET deleted_at=$2 WHERE id=$1")
            .bind(id)
            .bind(now)
            .execute(&mut *domain)
            .await?;
        if let Some(comment_id) = r.comment_id {
            sqlx::query("UPDATE porchlight.comments SET edited_at=$2 WHERE id=$1")
                .bind(comment_id)
                .bind(now)
                .execute(&mut *domain)
                .await?;
        }
        Ok(StatusCode::NO_CONTENT.into_response())
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
/// Transaction-local metadata contribution to the shared repeatable-read snapshot.
pub async fn snapshot_attachments_s4b(
    conn: &mut PgConnection,
    c: &Caller,
    item_ids: &[Uuid],
) -> Result<Vec<Attachment>, ApiError> {
    collab::read_items(conn, c, item_ids).await?;
    let sql = format!(
        "{ATTACHMENT_SELECT} WHERE {VISIBLE} AND {LIVE_ATTACHMENT} AND i.id=ANY($4) ORDER BY a.id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(c.member_id())
        .bind(c.sees_all())
        .bind(item_ids)
        .fetch_all(conn)
        .await?)
}

const LOCK_STORAGE_SHARD: &str =
    "SELECT shard FROM porchlight.blob_storage_locks WHERE shard=$1 FOR UPDATE";
const UPSERT_BLOB_METADATA: &str = "INSERT INTO porchlight.blobs(household_id,sha256,size,content_type) VALUES($1,$2,$3,$4) ON CONFLICT(household_id,sha256) DO UPDATE SET last_referenced_at=$5,size=EXCLUDED.size,content_type=EXCLUDED.content_type";
const INSERT_ATTACHMENT: &str = "INSERT INTO porchlight.attachments(household_id,item_id,comment_id,blob_sha256,thumbnail_sha256,original_name,uploader_id,uploader_token_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id";
const DOWNLOAD_BLOB: &str = "SELECT b.sha256,b.content_type,b.size FROM porchlight.attachments a JOIN porchlight.blobs b ON b.household_id=a.household_id AND b.sha256=CASE WHEN $3 THEN a.thumbnail_sha256 ELSE a.blob_sha256 END WHERE a.household_id=$1 AND a.id=$2";
