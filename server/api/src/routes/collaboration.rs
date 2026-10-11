//! Comments and attachments on both authenticated surfaces.
use crate::{
    blobs::{self, Attachment, Upload},
    collaboration::{self as service, Comment, CommentBody, CommentOp},
    error::ApiError,
    extract::Caller,
    idempotency::IdemTx,
    pagination::{Page, PageParams},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    response::Response,
};
use futures_util::future::BoxFuture;
use serde_json::Value;
use uuid::Uuid;
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct CommentPage {
    pub items: Vec<Comment>,
    pub next_cursor: Option<String>,
}
#[derive(utoipa::ToSchema)]
pub struct UploadSchema {
    #[schema(value_type=String,format=Binary)]
    pub file: String,
}

#[utoipa::path(get,path="/v1/items/{id}/comments",params(("id"=Uuid,Path),("cursor"=Option<String>,Query),("limit"=Option<u16>,Query)),responses((status=200,description="Visible comments in UUID order",body=CommentPage),crate::openapi::UniversalProblems,crate::routes::items::ReadValidationProblem,crate::openapi::NotFoundProblem,crate::openapi::ForbiddenProblem))]
pub async fn list_comments(
    State(state): State<AppState>,
    c: Caller,
    Path(id): Path<Uuid>,
    Query(q): Query<PageParams>,
) -> Result<Json<Page<Comment>>, ApiError> {
    // One read-only snapshot for the ACL check and the rows: a revocation (and anything written
    // after it) committed between the two statements must not reach a now-unauthorized reader.
    let mut conn = crate::items::read_snapshot(&state).await?;
    service::read_item(&mut conn, &c, id).await?;
    let context = format!("comments:{}:{}:{}", c.household_id(), c.member_id(), id);
    let after: Option<Uuid> = q
        .cursor
        .as_deref()
        .map(|v| state.idem.cursor.decode(&context, v))
        .transpose()?;
    let limit = q.limit()?;
    let rows = sqlx::query_as::<_, Comment>(LIST_LIVE_COMMENTS_AFTER)
        .bind(id)
        .bind(after)
        .bind((limit + 1) as i64)
        .fetch_all(&mut *conn)
        .await?;
    conn.commit().await?;
    Ok(Json(state.idem.cursor.page(
        &context,
        rows,
        limit,
        |r| r.id,
    )?))
}
#[utoipa::path(post,path="/v1/items/{id}/comments",params(("id"=Uuid,Path)),request_body=CommentBody,responses((status=201,description="Comment created",body=Comment),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem,crate::openapi::ForbiddenProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem))]
pub fn create_comment<'a>(
    s: &'a AppState,
    c: Caller,
    t: &'a mut IdemTx,
    b: CommentBody,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(service::mutate(s, c, t, Some(b), CommentOp::Create))
}
#[utoipa::path(patch,path="/v1/comments/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=CommentBody,responses((status=200,description="Comment updated",body=Comment),(status=403,body=crate::openapi::ProblemBody,description="Forbidden or comment-edit-expired",content_type="application/problem+json"),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::routes::items::IfMatchProblem,crate::openapi::RateLimitProblem))]
pub fn patch_comment<'a>(
    s: &'a AppState,
    c: Caller,
    t: &'a mut IdemTx,
    b: CommentBody,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(service::mutate(s, c, t, Some(b), CommentOp::Patch))
}
#[utoipa::path(delete,path="/v1/comments/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),responses((status=204,description="Soft deletion succeeded"),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem,crate::openapi::ForbiddenProblem,crate::openapi::MutationProblems,crate::routes::items::IfMatchProblem,crate::openapi::RateLimitProblem))]
pub fn delete_comment<'a>(
    s: &'a AppState,
    c: Caller,
    t: &'a mut IdemTx,
    _b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(service::mutate(s, c, t, None, CommentOp::Delete))
}
#[utoipa::path(post,path="/v1/items/{id}/attachments",params(("id"=Uuid,Path)),request_body(content=UploadSchema,content_type="multipart/form-data"),responses((status=201,description="Attachment metadata; stored idempotency response",body=Attachment),(status=413,description="attachment-too-large",content_type="application/problem+json",body=crate::openapi::ProblemBody),(status=415,description="attachment-type-unsupported; WebP main meta-Huffman is not supported",content_type="application/problem+json",body=crate::openapi::ProblemBody),(status=507,description="storage-full: household quota or free disk reserve exhausted",content_type="application/problem+json",body=crate::openapi::ProblemBody),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem,crate::openapi::ForbiddenProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem))]
pub fn upload_item<'a>(
    s: &'a AppState,
    c: Caller,
    t: &'a mut IdemTx,
    b: Upload,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(blobs::upload(s, c, t, b, false))
}
#[utoipa::path(post,path="/v1/comments/{id}/attachments",params(("id"=Uuid,Path)),request_body(content=UploadSchema,content_type="multipart/form-data"),responses((status=201,description="Attachment metadata; stored idempotency response",body=Attachment),(status=413,description="attachment-too-large",content_type="application/problem+json",body=crate::openapi::ProblemBody),(status=415,description="attachment-type-unsupported; WebP main meta-Huffman is not supported",content_type="application/problem+json",body=crate::openapi::ProblemBody),(status=507,description="storage-full: household quota or free disk reserve exhausted",content_type="application/problem+json",body=crate::openapi::ProblemBody),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem,crate::openapi::ForbiddenProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem))]
pub fn upload_comment<'a>(
    s: &'a AppState,
    c: Caller,
    t: &'a mut IdemTx,
    b: Upload,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(blobs::upload(s, c, t, b, true))
}
#[utoipa::path(get,path="/v1/attachments/{id}",params(("id"=Uuid,Path)),responses((status=200,description="Stored bytes; sanitized filename, nosniff",content((String="image/jpeg"),(String="image/png"),(String="image/gif"),(String="image/webp"),(String="application/pdf"),(String="text/plain; charset=utf-8"))),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem,crate::openapi::ForbiddenProblem))]
pub async fn download(
    State(s): State<AppState>,
    c: Caller,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    blobs::download(&s, c, id, false).await
}
#[utoipa::path(get,path="/v1/attachments/{id}/thumbnail",params(("id"=Uuid,Path)),responses((status=200,description="Thumbnail bytes; nosniff",content((String="image/jpeg"),(String="image/png"))),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem,crate::openapi::ForbiddenProblem))]
pub async fn thumbnail(
    State(s): State<AppState>,
    c: Caller,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    blobs::download(&s, c, id, true).await
}
#[utoipa::path(delete,path="/v1/attachments/{id}",description="Item attachment: uploader or item editor. Comment attachment: comment author within 15 minutes or super admin moderation; bumps the comment version.",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),responses((status=204,description="Soft deletion succeeded"),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem,crate::openapi::ForbiddenProblem,crate::openapi::MutationProblems,crate::routes::items::IfMatchProblem,crate::openapi::RateLimitProblem))]
pub fn delete_attachment<'a>(
    s: &'a AppState,
    c: Caller,
    t: &'a mut IdemTx,
    _b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(blobs::delete(s, c, t))
}
#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_comments,
    create_comment,
    patch_comment,
    delete_comment,
    upload_item,
    upload_comment,
    download,
    thumbnail,
    delete_attachment
))]
pub struct CollaborationApi;

// Every base registry operation has a row in the existing generator.
use crate::permissions::{Endpoint, Policy, PolicyContext};
use porchlight_core::access::{self, CallerKind, Capability, Role};
fn view_collab_policy(c: &PolicyContext) -> u16 {
    Policy::Item(Capability::View).expected(c)
}
fn create_collab_policy(c: &PolicyContext) -> u16 {
    match Policy::Item(Capability::Comment).expected(c) {
        200 => 201,
        n => n,
    }
}
fn author_only(c: &PolicyContext) -> u16 {
    if !access::can_view(&c.item) {
        404
    } else if c.is_self {
        200
    } else {
        403
    }
}
fn attach_comment_policy(c: &PolicyContext) -> u16 {
    match author_only(c) {
        200 => 201,
        status => status,
    }
}
fn delete_comment_policy(c: &PolicyContext) -> u16 {
    if !access::can_view(&c.item) {
        404
    } else if c.is_self
        || matches!(
            c.item.caller,
            CallerKind::Agent | CallerKind::AppMember(Role::SuperAdmin)
        )
    {
        204
    } else {
        403
    }
}
fn delete_attachment_policy(c: &PolicyContext) -> u16 {
    if !access::can_view(&c.item) {
        404
    } else if c.is_self || access::capabilities(&c.item).contains(Capability::Edit) {
        204
    } else {
        403
    }
}
pub const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/v1/items/{id}/comments",
        handler: "list_comments",
        policy: Policy::ItemResource {
            label: "view",
            expected: view_collab_policy,
        },
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/items/{id}/comments",
        handler: "create_comment",
        policy: Policy::ItemResource {
            label: "comment",
            expected: create_collab_policy,
        },
        case: "",
    },
    Endpoint {
        method: "PATCH",
        path: "/v1/comments/{id}",
        handler: "patch_comment",
        policy: Policy::ItemResource {
            label: "author within 15 minutes",
            expected: author_only,
        },
        case: "",
    },
    Endpoint {
        method: "DELETE",
        path: "/v1/comments/{id}",
        handler: "delete_comment",
        policy: Policy::ItemResource {
            label: "author or visible super-admin",
            expected: delete_comment_policy,
        },
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/items/{id}/attachments",
        handler: "upload_item",
        policy: Policy::ItemResource {
            label: "comment",
            expected: create_collab_policy,
        },
        case: "",
    },
    Endpoint {
        method: "POST",
        path: "/v1/comments/{id}/attachments",
        handler: "upload_comment",
        policy: Policy::ItemResource {
            label: "author within 15 minutes",
            expected: attach_comment_policy,
        },
        case: "",
    },
    Endpoint {
        method: "GET",
        path: "/v1/attachments/{id}",
        handler: "download",
        policy: Policy::ItemResource {
            label: "view",
            expected: view_collab_policy,
        },
        case: "",
    },
    Endpoint {
        method: "GET",
        path: "/v1/attachments/{id}/thumbnail",
        handler: "thumbnail",
        policy: Policy::ItemResource {
            label: "view",
            expected: view_collab_policy,
        },
        case: "",
    },
    Endpoint {
        method: "DELETE",
        path: "/v1/attachments/{id}",
        handler: "delete_attachment",
        policy: Policy::ItemResource {
            label: "item attachment: uploader or editor",
            expected: delete_attachment_policy,
        },
        case: "",
    },
    Endpoint {
        method: "DELETE",
        path: "/v1/attachments/{id}",
        handler: "delete_attachment",
        policy: Policy::ItemResource {
            label: "comment attachment: author within 15 minutes or super admin",
            expected: delete_comment_policy,
        },
        case: "comment-attachment",
    },
];

const LIST_LIVE_COMMENTS_AFTER: &str = concat!(
    "SELECT ",
    service::comment_columns!(),
    " FROM porchlight.comments WHERE item_id=$1 AND deleted_at IS NULL AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT $3"
);
