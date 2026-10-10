//! Authenticated ID-only tickles; each delivery/replay rechecks current ACLs.
use crate::{error::ApiError, extract::SessionAuth, server::ConnInfo, state::AppState};
use axum::{
    body::Body,
    extract::{Extension, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use porchlight_db::{auth, stream};
use serde::Deserialize;
use std::{collections::VecDeque, convert::Infallible};
use tokio::{
    sync::broadcast,
    time::{Instant, Interval, MissedTickBehavior},
};
use uuid::Uuid;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct Cursor {
    after: Option<String>,
}
struct Live {
    state: AppState,
    auth: SessionAuth,
    fingerprint: [u8; 32],
    session_hash: [u8; 32],
    cursor: i64,
    epoch: i64,
    pending: VecDeque<stream::Tickle>,
    opening: Option<String>,
    shutdown_sent: bool,
    terminal: bool,
    first_frame: bool,
    drain: bool,
    heartbeats: Interval,
    heartbeat_at: Instant,
    revocations: broadcast::Receiver<Uuid>,
    wakeups: broadcast::Receiver<()>,
}
impl Live {
    async fn validate(&self) -> sqlx::Result<bool> {
        // Preserve opening database failures so they return a problem before SSE headers.
        // Heartbeats use the same best-effort device-before-session sliding as requests.
        let Some(d) = auth::device_by_fingerprint(&self.state.pool, &self.fingerprint).await?
        else {
            return Ok(false);
        };
        if d.revoked || !d.certificate_live || d.device_id != self.auth.device.device_id {
            return Ok(false);
        }
        let Some(s) = auth::find_session(&self.state.pool, &self.session_hash).await? else {
            return Ok(false);
        };
        if s.id != self.auth.session_id || s.device_id != d.device_id {
            return Ok(false);
        }
        use porchlight_core::auth::session::{
            Platform, SESSION_TOUCH_INTERVAL_SECS, session_lifetime,
        };
        let now = (self.state.item_clock)();
        if (now - s.last_used_at).num_seconds() >= SESSION_TOUCH_INTERVAL_SECS {
            let Some(platform) = Platform::parse(&d.platform) else {
                return Ok(false);
            };
            let mut tx = porchlight_db::tx::begin_as(
                &self.state.pool,
                &crate::extract::actor(&d, self.auth.request_id),
            )
            .await?;
            auth::touch_session(&mut *tx, s.id, now + session_lifetime(platform)).await?;
            let live = matches!(auth::find_session(&mut *tx, &self.session_hash).await?, Some(current) if current.id==s.id && current.device_id==d.device_id);
            tx.commit().await?;
            return Ok(live);
        }
        Ok(true)
    }
    async fn valid(&self) -> bool {
        // Once headers are sent, either invalid credentials or a DB failure closes the body.
        self.validate().await.unwrap_or(false)
    }
    async fn revocation(&self, hint: Result<Uuid, broadcast::error::RecvError>) -> bool {
        match hint {
            Ok(id) => id != self.auth.device.device_id,
            Err(broadcast::error::RecvError::Lagged(_)) => self.valid().await,
            Err(broadcast::error::RecvError::Closed) => false,
        }
    }
    async fn next(&mut self) -> Option<String> {
        loop {
            if self.state.hub.is_draining() && !self.shutdown_sent {
                self.shutdown_sent = true;
                self.terminal = true;
                self.opening = None;
                self.pending.clear();
                return Some(control("drain", r#"{"reason":"shutdown"}"#));
            }
            if !self.terminal
                && stream::retention_epoch(&self.state.pool).await.ok() != Some(self.epoch)
            {
                self.terminal = true;
                self.pending.clear();
                self.opening = None;
                return Some(control("resnapshot", r#"{"reason":"retention_pruned"}"#));
            }
            // A queued ready/replay frame must not hide an already-received revocation.
            match self.revocations.try_recv() {
                Ok(id) if id == self.auth.device.device_id => return None,
                Ok(_) => continue,
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    if !self.valid().await {
                        return None;
                    }
                    continue;
                }
                Err(broadcast::error::TryRecvError::Closed) => return None,
                Err(broadcast::error::TryRecvError::Empty) => {}
            }
            if self.terminal {
                return self.opening.take();
            }
            if self.first_frame {
                self.first_frame = false;
                // Even a slow initial validation must not put a heartbeat before ready.
                self.heartbeats.reset();
                self.heartbeat_at = Instant::now() + self.state.settings.heartbeat;
                return self.opening.take();
            }
            if self.heartbeat_at <= Instant::now() {
                self.heartbeats.tick().await;
                self.heartbeat_at = Instant::now() + self.state.settings.heartbeat;
                if !self.valid().await {
                    return None;
                }
                self.drain = true;
                return Some(": hb\n\n".to_owned());
            }
            if !self.pending.is_empty() {
                let seqs: Vec<_> = self.pending.iter().map(|t| t.seq).collect();
                let result = tokio::select! {
                    biased;
                    ()=self.state.hub.draining()=>continue,
                    hint=self.revocations.recv()=>{
                        if !self.revocation(hint).await { return None; }
                        continue;
                    },
                    r=stream::delivery_batch(&self.state.pool,self.auth.device.household_id,self.auth.device.member_id,&seqs)=>r,
                };
                self.pending.clear();
                match result {
                    Ok(batch) if batch.epoch != self.epoch => {
                        self.terminal = true;
                        return Some(control("resnapshot", r#"{"reason":"retention_pruned"}"#));
                    }
                    Ok(batch) if !batch.events.is_empty() => {
                        // One flush = one recheck = one body chunk. Never cache an ACL
                        // decision across polls (a slow socket only retains encoded bytes).
                        return Some(tickle_chunk(batch.events));
                    }
                    Ok(_) => continue,
                    Err(error) => {
                        tracing::warn!(
                            class = error_class(&error),
                            "stream delivery failed; closing"
                        );
                        return None;
                    }
                }
            }
            if self.drain {
                let result = tokio::select! {
                    biased;
                    ()=self.state.hub.draining()=>continue,
                    hint=self.revocations.recv()=>{ if !self.revocation(hint).await { return None; } continue; },
                    _=self.heartbeats.tick()=>{
                        self.heartbeat_at=Instant::now()+self.state.settings.heartbeat;
                        if !self.valid().await { return None; }
                        // The cancelled read resumes after this heartbeat, from the same cursor.
                        return Some(": hb\n\n".to_owned());
                    },
                    r=stream::read(&self.state.pool,self.auth.device.household_id,self.auth.device.member_id,self.cursor)=>r,
                };
                let Ok(batch) = result else {
                    tracing::warn!("stream outbox read failed; closing");
                    return None;
                };
                if batch.epoch != self.epoch {
                    self.terminal = true;
                    return Some(control("resnapshot", r#"{"reason":"retention_pruned"}"#));
                }
                self.cursor = batch.cursor;
                self.drain = batch.scanned == stream::BATCH;
                self.pending.extend(batch.events);
                continue;
            }
            tokio::select! {
                biased;
                ()=self.state.hub.draining()=>continue,
                hint=self.revocations.recv()=>{ if !self.revocation(hint).await { return None; } },
                _=self.heartbeats.tick()=>{
                    self.heartbeat_at=Instant::now()+self.state.settings.heartbeat;
                    if !self.valid().await { return None; }
                    self.drain=true;
                    return Some(": hb\n\n".to_owned());
                },
                hint=self.wakeups.recv()=>{
                    if matches!(hint,Err(broadcast::error::RecvError::Closed)) { return None; }
                    // Lagging is harmless: we always read after our own cursor.
                    self.drain=true;
                },
            }
        }
    }
}
/// One mTLS+session stream. Tickle data is StreamTickle (IDs/categories only).
/// Item/occurrence/derived/comment/attachment use current item ACL; lists use item
/// scope or pl_list_can_read; notifications/settings/feed/suggestions use member
/// equality; member.changed is household-wide. Access changes for former readers
/// carry evicted=true and must remove the entity and its dependent caches.
/// Heartbeats are comments every 120 seconds. Ready has no ID and returns the scan
/// cursor. Last-Event-ID takes precedence over after. Cursors older than seven
/// days of outbox creation time, pruned prefixes, or above the global maximum get
/// a terminal resnapshot event. Shutdown emits drain and clean EOF; reconnect
/// using the last tickle ID. No control frame advances the replay cursor.
#[utoipa::path(get,
    path="/v1/stream",
    params(("Last-Event-ID"=Option<i64>,Header,description="Resume after the stream cursor"),("after"=Option<i64>,Query,description="Resume cursor when Last-Event-ID is absent")),
    responses((status=200,description="Success",body=String,content_type="text/event-stream"),crate::openapi::UniversalProblems),
    security(("session" = [])))]
pub async fn get(
    State(state): State<AppState>,
    auth: SessionAuth,
    Extension(conn): Extension<ConnInfo>,
    headers: HeaderMap,
    Query(query): Query<Cursor>,
) -> Result<Response, ApiError> {
    #[cfg(feature = "testing")]
    if !state.stream_pre_subscribe_delay.is_zero() {
        state.stream_pre_subscribe_started.notify_one();
        tokio::time::sleep(state.stream_pre_subscribe_delay).await;
    }
    // Subscribe BEFORE database re-validation: no extractor → subscription gap.
    let revocations = state.revocations.subscribe();
    let wakeups = state.hub.subscribe();
    let fingerprint = conn
        .client_cert_fingerprint
        .ok_or_else(|| ApiError::new(crate::error::Problem::Unauthenticated))?;
    let session_hash = auth.session_hash;
    let cursor = headers
        .get("last-event-id")
        .map(|h| {
            h.to_str()
                .map(str::to_owned)
                .map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))
        })
        .transpose()?
        .or(query.after);
    let cursor = cursor
        .map(|v| {
            v.parse::<i64>()
                .ok()
                .filter(|n| *n >= 0)
                .ok_or_else(|| ApiError::new(crate::error::Problem::InvalidRequest))
        })
        .transpose()?;
    let heartbeat_at = Instant::now() + state.settings.heartbeat;
    let mut heartbeats = tokio::time::interval_at(heartbeat_at, state.settings.heartbeat);
    heartbeats.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut live = Live {
        state,
        auth,
        fingerprint,
        session_hash,
        cursor: 0,
        epoch: 0,
        pending: VecDeque::new(),
        opening: None,
        shutdown_sent: false,
        terminal: false,
        first_frame: true,
        drain: true,
        heartbeats,
        heartbeat_at,
        revocations,
        wakeups,
    };
    if !live.validate().await? {
        live.terminal = true;
    } else {
        let window = stream::window_at(&live.state.pool, (live.state.item_clock)()).await?;
        live.epoch = window.epoch;
        live.cursor = cursor.unwrap_or(window.maximum);
        if live.cursor > window.maximum || cursor.is_some_and(|after| window.expired(after)) {
            live.terminal = true;
            let reason = if live.cursor > window.maximum {
                "cursor_unknown"
            } else {
                "cursor_expired"
            };
            live.opening = Some(control(
                "resnapshot",
                &format!(r#"{{"reason":"{reason}"}}"#),
            ));
        } else {
            live.opening = Some(control(
                "ready",
                &format!(r#"{{"cursor":{}}}"#, live.cursor),
            ));
        }
    }
    let body = Body::from_stream(futures_util::stream::unfold(live, |mut live| async move {
        let chunk = live.next().await?;
        Some((Ok::<_, Infallible>(chunk), live))
    }));
    Ok((
        [
            ("content-type", "text/event-stream"),
            ("cache-control", "no-cache"),
        ],
        body,
    )
        .into_response())
}
fn control(kind: &str, data: &str) -> String {
    format!("event: {kind}\ndata: {data}\n\n")
}
fn tickle_chunk(events: Vec<stream::Tickle>) -> String {
    use std::fmt::Write;
    let mut chunk = String::new();
    for t in events {
        // Only the fixed routed catalogue is delivered; no untrusted event names.
        writeln!(
            chunk,
            "id: {}\nevent: {}\ndata: {}\n",
            t.seq,
            t.kind,
            serde_json::to_string(&t).expect("tickle serializes")
        )
        .expect("string write");
    }
    chunk
}
fn error_class(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::Database(_) => "database",
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed => "pool",
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) => "transport",
        _ => "decode_or_other",
    }
}

/// JSON data of an SSE tickle (not a full entity response).
#[derive(utoipa::ToSchema)]
pub struct StreamTickle {
    pub seq: i64,
    pub kind: String,
    pub entity_type: String,
    pub entity_id: Uuid,
    /// Omitted unless true. Remove cached entity; an item eviction also removes children.
    pub evicted: Option<bool>,
}
#[derive(utoipa::ToSchema)]
pub struct StreamReady {
    pub cursor: i64,
}
#[derive(utoipa::ToSchema)]
pub struct StreamControl {
    pub reason: String,
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(get),
    components(schemas(StreamTickle, StreamReady, StreamControl))
)]
pub struct StreamApi;

#[cfg(test)]
mod tests {
    #[test]
    fn a_flush_encodes_all_500_tickles_in_one_chunk() {
        let events = (1..=500)
            .map(|seq| porchlight_db::stream::Tickle {
                seq,
                kind: "item.changed".into(),
                entity_type: "item".into(),
                entity_id: uuid::Uuid::nil(),
                evicted: false,
            })
            .collect();
        let chunk = super::tickle_chunk(events);
        let frames: Vec<_> = chunk.split("\n\n").filter(|f| !f.is_empty()).collect();
        assert_eq!(frames.len(), 500);
        for (index, frame) in frames.iter().enumerate() {
            assert!(frame.starts_with(&format!("id: {}\nevent: item.changed\ndata: ", index + 1)));
            let data = frame.split("data: ").nth(1).unwrap();
            let data: serde_json::Value = serde_json::from_str(data).unwrap();
            assert_eq!(data["seq"], index + 1);
            assert!(data.get("evicted").is_none());
        }
    }
}
