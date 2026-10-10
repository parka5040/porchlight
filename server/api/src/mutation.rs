//! One mutation lifecycle: authenticated claim/revalidation, handler, sealed response, commit.
//! Unexpected and explicitly transient errors roll back the actor transaction and its claim.
use crate::{
    error::{ApiError, Problem},
    extract::{Caller, Mutation},
    idempotency::{self, Begin, IdemScope, IdemTx},
    state::AppState,
};
use axum::{
    extract::Request,
    handler::Handler,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::MethodRouter,
};
use porchlight_db::idempotency::ScopeKind;
use std::future::Future;

/// Both ordinary claims and multipart replay probes use identical auth/scope preparation.
fn write_scope(caller: &Caller, auth: &mut crate::extract::MutationAuth) -> IdemScope {
    if let crate::extract::MutationAuth::Pat { write, .. } = auth {
        *write = true;
    }
    let (kind, scope_id) = match caller {
        Caller::Member { device_id, .. } => (ScopeKind::Device, *device_id),
        Caller::Agent { token_id, .. } => (ScopeKind::Pat, *token_id),
    };
    IdemScope {
        kind,
        scope_id,
        household_id: caller.household_id(),
    }
}

pub(crate) async fn require_key(req: Request, next: Next) -> Result<Response, ApiError> {
    if matches!(
        *req.method(),
        axum::http::Method::POST
            | axum::http::Method::PUT
            | axum::http::Method::PATCH
            | axum::http::Method::DELETE
    ) {
        crate::extract::idempotency_key(req.headers())?;
    }
    Ok(next.run(req).await)
}
/// Explicit compatibility escape hatch for the thirteen pinned M2 trust adapters only.
/// These adapters have special session-less authentication and call execute themselves.
pub(crate) fn legacy_post<H, T>(handler: H) -> MethodRouter<AppState>
where
    H: Handler<T, AppState>,
    T: 'static,
{
    axum::routing::post(handler).layer(middleware::from_fn(require_key))
}
/// Shared terminal lifecycle, also used by the session-less login and enrollment adapters.
pub async fn execute<F, Fut>(
    state: &AppState,
    begin: Begin,
    handler: F,
) -> Result<Response, ApiError>
where
    F: FnOnce(IdemTx) -> Fut,
    Fut: Future<Output = Result<(IdemTx, Response), ApiError>>,
{
    match begin {
        Begin::Replay(response) => Ok(response),
        Begin::Fresh(tx) => {
            let (tx, response) = handler(tx).await?;
            tx.finish(state, response).await
        }
    }
}
/// Generic attributed mutation for later domain routes on either application or agent routers.
/// The body is parsed by Mutation; header authentication by Caller; authority is checked again
/// after the claim wait in begin. Agent tokens need write or admin scope.
/// Callbacks borrow AppState and IdemTx for the same lifetime. Ordinary business
/// errors are sealed; ApiError::transient and retryable SQLSTATEs roll back without
/// storing a response, allowing the same key/body to execute again.
/// All callback SQL (including reads and helper calls) MUST use `&mut *tx.tx`,
/// never `state.pool` or a separate connection/transaction: otherwise effects escape
/// rollback/replay and locks may self-block. State is for settings, limits, hub and
/// scheduler access, not SQL. Callbacks must not begin, seal or commit themselves.
pub async fn run<T, F>(
    state: &AppState,
    caller: Caller,
    m: Mutation<T>,
    handler: F,
) -> Result<Response, ApiError>
where
    F: for<'a> FnOnce(
        &'a AppState,
        Caller,
        &'a mut IdemTx,
        T,
    ) -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>>,
{
    run_inner(state, caller, m, handler, false).await
}
fn check_write_admission(state: &AppState, caller: &Caller) -> Result<(), ApiError> {
    if let Caller::Agent { scopes, .. } = caller
        && !crate::extract::pat_allows_scope(scopes, "write")
    {
        return Err(ApiError::new(Problem::Forbidden));
    }
    if let Caller::Member {
        device_id,
        client_addr,
        ..
    } = caller
    {
        state.limits.check(
            crate::ratelimit::Class::Write,
            Some(*device_id),
            *client_addr,
        )?;
    }
    Ok(())
}
async fn run_inner<T, F>(
    state: &AppState,
    mut caller: Caller,
    mut m: Mutation<T>,
    handler: F,
    admitted: bool,
) -> Result<Response, ApiError>
where
    F: for<'a> FnOnce(
        &'a AppState,
        Caller,
        &'a mut IdemTx,
        T,
    ) -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>>,
{
    if !admitted {
        check_write_admission(state, &caller)?;
    }
    let scope = write_scope(&caller, &mut m.auth);
    let begin = idempotency::begin(
        state,
        &caller.actor(),
        &m.auth,
        scope,
        &m.key,
        &m.request_hash,
    )
    .await?;
    let mut tx = match begin {
        Begin::Replay(r) => return Ok(r),
        Begin::Fresh(tx) => tx,
    };
    // A PAT's scopes can change between headers and the transaction. Handlers see
    // the locked current scopes, not the header-time copy (including admin decisions).
    if let Caller::Agent {
        token_id, scopes, ..
    } = &mut caller
    {
        let (_, current) = porchlight_db::tokens::share_pat(&mut tx.tx, *token_id)
            .await?
            .ok_or_else(|| ApiError::new(Problem::Unauthenticated))?;
        *scopes = current;
    }
    tx.context = Box::new(m.context);
    let response = match handler(state, caller, &mut tx, m.body).await {
        Ok(r) => r,
        Err(e) if e.is_transient() => {
            // Explicit rollback also discards claim, consumed step-up and queued
            // post-commit publications. Never run sealing SQL in an aborted tx.
            tx.tx.rollback().await?;
            return Ok(e.into_response());
        }
        Err(e) => e.into_response(),
    };
    tx.finish(state, response).await
}

/// The constructor owns authentication, body parsing and the whole transaction lifecycle.
/// A callback cannot bypass revalidation, claim, response sealing or commit.
macro_rules! register_mutation {
    ($($method:ident),*) => {$(
        pub fn $method<T, F>(handler: F) -> MethodRouter<AppState>
        where
            T: serde::de::DeserializeOwned + Send + 'static,
            F: for<'a> Fn(&'a AppState, Caller, &'a mut IdemTx, T)
                -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>>
                + Clone + Send + Sync + 'static,
        {
            axum::routing::$method(move |axum::extract::State(state): axum::extract::State<AppState>, caller: Caller, m: Mutation<T>| {
                let handler = handler.clone();
                async move { run(&state, caller, m, handler).await }
            }).layer(middleware::from_fn(require_key))
        }
    )*}
}
register_mutation!(post, put, patch, delete);

/// S7b: the same claim/revalidation/seal lifecycle, with a bounded streamed multipart extractor.
pub fn multipart<F>(handler: F) -> MethodRouter<AppState>
where
    F: for<'a> Fn(
            &'a AppState,
            Caller,
            &'a mut IdemTx,
            crate::blobs::Upload,
        ) -> futures_util::future::BoxFuture<'a, Result<Response, ApiError>>
        + Clone
        + Send
        + Sync
        + 'static,
{
    axum::routing::post(
        move |axum::extract::State(state): axum::extract::State<AppState>,
              caller: Caller,
              req: Request| {
            let handler = handler.clone();
            async move {
                check_write_admission(&state, &caller)?;
                let mut m = <crate::blobs::UploadMutation as axum::extract::FromRequest<
                    AppState,
                >>::from_request(req, &state)
                .await?;
                let scope = write_scope(&caller, &mut m.0.auth);
                if let Some(replay) = idempotency::replay_if_completed(
                    &state,
                    &caller.actor(),
                    &m.0.auth,
                    scope,
                    &m.0.key,
                    &m.0.request_hash,
                )
                .await?
                {
                    return Ok(replay);
                }
                crate::blobs::admit_ingest(&state, &mut m.0.body).await?;
                run_inner(&state, caller, m.0, handler, true).await
            }
        },
    )
    .layer(axum::extract::DefaultBodyLimit::max(
        crate::blobs::MAX_UPLOAD + 64 * 1024,
    ))
    .layer(middleware::from_fn(require_key))
}
