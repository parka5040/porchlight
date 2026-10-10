//! Claim-first, transaction-atomic idempotency. Authenticate before calling `begin`, including terminal retries.
use crate::{error::ApiError, state::AppState};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use axum::{
    body::{Body, to_bytes},
    http::{HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use chrono::Duration;
use hmac::{Hmac, Mac};
use porchlight_core::auth::token::hashes_equal;
use porchlight_db::{
    idempotency::{self, ClaimOutcome, ScopeKind},
    tx::{Actor, begin_as},
};
use sha2::Sha256;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroizing;

const HEADERS: &[&str] = &["content-type", "retry-after", "location", "etag"];
const RESPONSE_LIMIT: usize = 1024 * 1024;
type Envelope = (Vec<(String, String)>, Vec<u8>);
pub struct IdempotencyKeys {
    pub cursor: crate::pagination::CursorKeys,
    enc: Aes256Gcm,
    mac: Zeroizing<[u8; 32]>,
}
impl IdempotencyKeys {
    pub fn new(master: &[u8; 32]) -> Self {
        let hkdf = hkdf::Hkdf::<Sha256>::new(Some(b""), master);
        let mut enc = Zeroizing::new([0; 32]);
        let mut mac = Zeroizing::new([0; 32]);
        hkdf.expand(b"porchlight/idempotency/enc", &mut *enc)
            .expect("valid HKDF length");
        hkdf.expand(b"porchlight/idempotency/mac", &mut *mac)
            .expect("valid HKDF length");
        Self {
            cursor: crate::pagination::CursorKeys::new(master),
            enc: Aes256Gcm::new_from_slice(&*enc).expect("AES key length"),
            mac,
        }
    }
    pub fn request_mac(&self, method: &str, path_and_query: &str, body: &[u8]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(&*self.mac)
            .expect("HMAC accepts this key");
        mac.update(method.as_bytes());
        mac.update(b"\n");
        mac.update(path_and_query.as_bytes());
        mac.update(b"\n");
        mac.update(body);
        mac.finalize().into_bytes().into()
    }
    /// Multipart semantic identity, bounded-memory equivalent of request_mac(prefix + file).
    pub fn request_mac_reader(
        &self,
        method: &str,
        path: &str,
        prefix: &[u8],
        reader: &mut impl std::io::Read,
    ) -> std::io::Result<[u8; 32]> {
        let mut mac =
            <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(&*self.mac).expect("HMAC key");
        mac.update(method.as_bytes());
        mac.update(b"\n");
        mac.update(path.as_bytes());
        mac.update(b"\n");
        mac.update(prefix);
        let mut chunk = [0u8; 8192];
        loop {
            let n = reader.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            mac.update(&chunk[..n]);
        }
        Ok(mac.finalize().into_bytes().into())
    }
    pub fn seal(&self, aad: &str, headers: &[(String, String)], body: &[u8]) -> Vec<u8> {
        let headers: Vec<_> = headers
            .iter()
            .filter(|(k, _)| HEADERS.contains(&k.as_str()))
            .cloned()
            .collect();
        // Binary envelope: big-endian header length, JSON headers, then raw body bytes.
        // Avoid serializing every body byte as a JSON number.
        let encoded_headers = serde_json::to_vec(&headers).expect("headers serialize");
        let mut plaintext =
            Zeroizing::new(Vec::with_capacity(4 + encoded_headers.len() + body.len()));
        plaintext.extend_from_slice(
            &u32::try_from(encoded_headers.len())
                .expect("bounded headers")
                .to_be_bytes(),
        );
        plaintext.extend_from_slice(&encoded_headers);
        plaintext.extend_from_slice(body);
        let mut nonce = [0; 12];
        getrandom::fill(&mut nonce).expect("OS random source failed");
        let encrypted = self
            .enc
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .expect("bounded response encryption");
        let mut sealed = nonce.to_vec();
        sealed.extend(encrypted);
        sealed
    }
    pub fn open(&self, aad: &str, sealed: &[u8]) -> Result<Envelope, ApiError> {
        if sealed.len() < 28 {
            return Err(ApiError::internal("idempotency_invalid_stored_response"));
        }
        let nonce: [u8; 12] = sealed[..12]
            .try_into()
            .map_err(|_| ApiError::internal("idempotency_invalid_stored_response"))?;
        let plaintext = Zeroizing::new(
            self.enc
                .decrypt(
                    &Nonce::from(nonce),
                    Payload {
                        msg: &sealed[12..],
                        aad: aad.as_bytes(),
                    },
                )
                .map_err(|_| ApiError::internal("idempotency_decryption"))?,
        );
        let size = plaintext
            .get(..4)
            .ok_or_else(|| ApiError::internal("idempotency_envelope_length"))?;
        let size = u32::from_be_bytes(size.try_into().expect("four bytes")) as usize;
        let end = 4usize
            .checked_add(size)
            .filter(|end| *end <= plaintext.len())
            .ok_or_else(|| ApiError::internal("idempotency_envelope_length"))?;
        let headers = serde_json::from_slice(&plaintext[4..end])
            .map_err(|_| ApiError::internal("idempotency_envelope_headers"))?;
        let envelope: Envelope = (headers, plaintext[end..].to_vec());
        if envelope
            .0
            .iter()
            .any(|(k, _)| !HEADERS.contains(&k.as_str()))
        {
            return Err(ApiError::internal("idempotency_invalid_stored_response"));
        }
        Ok(envelope)
    }
}
pub enum Begin {
    Replay(Response),
    Fresh(IdemTx),
}
pub struct IdemScope {
    pub kind: ScopeKind,
    pub scope_id: Uuid,
    pub household_id: Uuid,
}
impl IdemScope {
    fn aad(&self, key: &str) -> String {
        format!("{}:{}:{key}", self.kind.as_str(), self.scope_id)
    }
}
/// Best-effort work after a successful domain commit, never on replay or rollback.
/// The initial response is sealed atomically with domain effects as a crash-safe fallback.
/// Hooks may refresh the response (e.g. pending flags) in a fresh transaction. A concurrent
/// replay during the hook can receive the conservative initial response; no effects repeat.
/// Do not use hooks for authoritative writes or return a business failure after commit.
pub type AfterCommit = Box<
    dyn FnOnce(AppState, Response) -> futures_util::future::BoxFuture<'static, Response> + Send,
>;
#[derive(Default)]
struct PostCommit {
    revocations: Vec<(Uuid, Vec<[u8; 32]>)>,
    hooks: Vec<AfterCommit>,
}
#[derive(Clone, Default)]
pub struct RequestContext {
    pub uri: axum::http::Uri,
    /// UUID from Axum's matched path extractor, never parsed from the URI layout.
    pub target_id: Option<Uuid>,
    pub if_match: axum::http::HeaderMap,
}
pub struct IdemTx {
    pub tx: Transaction<'static, Postgres>,
    pub context: Box<RequestContext>,
    claim_id: Uuid,
    aad: String,
    auth: crate::extract::MutationAuth,
    after_commit: Box<PostCommit>,
}
#[allow(private_interfaces)] // Test-only low-level API; production auth is crate-private.
pub async fn begin(
    state: &AppState,
    actor: &Actor,
    auth: &crate::extract::MutationAuth,
    scope: IdemScope,
    key: &str,
    request_hash: &[u8; 32],
) -> Result<Begin, ApiError> {
    begin_inner(state, actor, auth, scope, key, request_hash, None).await
}
/// Token revocation must reserve its complete PAT set before revalidation locks
/// the caller. Other mutations continue to reserve only their own PAT/device.
pub(crate) async fn begin_pat_revoke(
    state: &AppState,
    actor: &Actor,
    auth: &crate::extract::MutationAuth,
    scope: IdemScope,
    key: &str,
    request_hash: &[u8; 32],
    target: Uuid,
) -> Result<Begin, ApiError> {
    begin_inner(state, actor, auth, scope, key, request_hash, Some(target)).await
}
async fn begin_inner(
    state: &AppState,
    actor: &Actor,
    auth: &crate::extract::MutationAuth,
    scope: IdemScope,
    key: &str,
    request_hash: &[u8; 32],
    revoke_target: Option<Uuid>,
) -> Result<Begin, ApiError> {
    use crate::extract::MutationAuth;
    let matches_scope = match auth {
        MutationAuth::Enrollment => matches!(scope.kind, ScopeKind::Enrollment),
        MutationAuth::Device { id, .. } => {
            matches!(scope.kind, ScopeKind::Device) && scope.scope_id == *id
        }
        MutationAuth::Pat { id, .. } => {
            matches!(scope.kind, ScopeKind::Pat) && scope.scope_id == *id
        }
    };
    if !matches_scope {
        return Err(ApiError::new(crate::error::Problem::Unauthenticated));
    }
    let mut tx = begin_as(&state.pool, actor).await?;
    let claim = idempotency::claim(
        &mut tx,
        scope.household_id,
        scope.kind,
        scope.scope_id,
        key,
        request_hash,
        Duration::hours(24),
    )
    .await
    .map_err(|e| {
        if e.as_database_error()
            .is_some_and(|e| e.code().as_deref() == Some("55P03"))
        {
            ApiError::new(crate::error::Problem::IdempotencyInProgress).retry(1)
        } else {
            ApiError::from(e)
        }
    })?;
    if let Some(target) = revoke_target {
        let MutationAuth::Pat { id, .. } = auth else {
            return Err(ApiError::new(crate::error::Problem::Unauthenticated));
        };
        porchlight_db::tokens::lock_revoke_pair(&mut tx, scope.household_id, *id, target).await?;
    }
    // Even replays must revalidate: body withholding and claim waits cannot preserve authority.
    revalidate(&mut tx, auth).await?;
    let aad = scope.aad(key);
    match claim {
        ClaimOutcome::Claimed(claim_id) => Ok(Begin::Fresh(IdemTx {
            tx,
            context: Box::default(),
            claim_id,
            aad,
            auth: auth.clone(),
            after_commit: Box::default(),
        })),
        ClaimOutcome::Existing(stored) => {
            let old: [u8; 32] = stored
                .request_hash
                .try_into()
                .map_err(|_| ApiError::internal("idempotency_invalid_stored_response"))?;
            if !hashes_equal(request_hash, &old) {
                return Err(ApiError::new(crate::error::Problem::IdempotencyKeyReused));
            }
            let status = u16::try_from(
                stored
                    .status
                    .ok_or_else(|| ApiError::internal("idempotency_invalid_stored_response"))?,
            )
            .map_err(|_| ApiError::internal("idempotency_invalid_stored_response"))?;
            let (headers, body) = state.idem.open(
                &aad,
                &stored
                    .sealed
                    .ok_or_else(|| ApiError::internal("idempotency_invalid_stored_response"))?,
            )?;
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = StatusCode::from_u16(status)
                .map_err(|_| ApiError::internal("idempotency_invalid_stored_response"))?;
            for (k, v) in headers {
                response.headers_mut().append(
                    HeaderName::try_from(k)
                        .map_err(|_| ApiError::internal("idempotency_invalid_stored_response"))?,
                    HeaderValue::try_from(v)
                        .map_err(|_| ApiError::internal("idempotency_invalid_stored_response"))?,
                );
            }
            tx.rollback().await?;
            Ok(Begin::Replay(response))
        }
    }
}
/// Multipart optimization: only probe committed keys before waiting for decoder admission.
/// Actual replay still uses the one claim/revalidation/open path above. A concurrently purged
/// key is rolled back here and claimed afresh after admission; no connection is held in the queue.
pub(crate) async fn replay_if_completed(
    state: &AppState,
    actor: &Actor,
    auth: &crate::extract::MutationAuth,
    scope: IdemScope,
    key: &str,
    request_hash: &[u8; 32],
) -> Result<Option<Response>, ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.idempotency_keys WHERE scope_kind=$1 AND scope_id=$2 AND idem_key=$3 AND household_id=$4 AND response_sealed IS NOT NULL)")
        .bind(scope.kind.as_str()).bind(scope.scope_id).bind(key).bind(scope.household_id).fetch_one(&state.pool).await?;
    if !exists {
        return Ok(None);
    }
    match begin(state, actor, auth, scope, key, request_hash).await? {
        Begin::Replay(response) => Ok(Some(response)),
        Begin::Fresh(tx) => {
            tx.tx.rollback().await?;
            Ok(None)
        }
    }
}
async fn revalidate(
    tx: &mut Transaction<'_, Postgres>,
    auth: &crate::extract::MutationAuth,
) -> Result<(), ApiError> {
    use crate::extract::MutationAuth;
    use porchlight_db::{auth, tokens};
    match auth {
        MutationAuth::Enrollment => {} // Token is authenticated and locked by enrollment itself.
        MutationAuth::Device {
            id,
            fingerprint,
            role,
            session,
            role_sensitive,
        } => {
            match auth::share_device(tx, *id).await? {
                None => return Err(ApiError::new(crate::error::Problem::DeviceUnknown)),
                Some(false) => return Err(ApiError::new(crate::error::Problem::DeviceRevoked)),
                Some(true) => {}
            }
            if let Some(session) = session
                && !auth::share_session(tx, *session, *id).await?
            {
                return Err(ApiError::new(crate::error::Problem::SessionInvalid));
            }
            let d = auth::device_by_fingerprint(&mut **tx, fingerprint)
                .await?
                .ok_or_else(|| ApiError::new(crate::error::Problem::DeviceUnknown))?;
            if !d.certificate_live {
                return Err(ApiError::new(
                    crate::error::Problem::DeviceCertificateExpired,
                ));
            }
            let current_role = auth::share_member(tx, d.member_id)
                .await?
                .ok_or_else(|| ApiError::new(crate::error::Problem::DeviceUnknown))?;
            if *role_sensitive && &current_role != role {
                return Err(ApiError::new(crate::error::Problem::Forbidden));
            }
        }
        MutationAuth::Pat { id, admin, write } => {
            let (member, scopes) = tokens::share_pat(tx, *id)
                .await?
                .ok_or_else(|| ApiError::new(crate::error::Problem::Unauthenticated))?;
            if auth::share_member(tx, member).await?.as_deref() != Some("super_admin") {
                return Err(ApiError::new(crate::error::Problem::Unauthenticated));
            }
            if *write && !crate::extract::pat_allows_scope(&scopes, "write") {
                return Err(ApiError::new(crate::error::Problem::Forbidden));
            }
            if *admin && !crate::extract::pat_allows_scope(&scopes, "admin") {
                return Err(ApiError::new(crate::error::Problem::Forbidden));
            }
        }
    }
    Ok(())
}

impl IdemTx {
    /// Register best-effort post-commit work. Not executed for replays, failures or rollbacks.
    pub fn after_commit(&mut self, hook: AfterCommit) {
        self.after_commit.hooks.push(hook);
    }
    pub(crate) fn publish_revocation(&mut self, id: Uuid, fingerprints: Vec<[u8; 32]>) {
        self.after_commit.revocations.push((id, fingerprints));
    }
    /// Recheck an unlocked session after acquiring credentials: another device's
    /// password change may have revoked it while this transaction waited.
    pub(crate) async fn recheck_session(&mut self) -> Result<(), ApiError> {
        if let crate::extract::MutationAuth::Device {
            id,
            session: Some(session),
            ..
        } = self.auth
            && !porchlight_db::auth::share_session(&mut self.tx, session, id).await?
        {
            return Err(ApiError::new(crate::error::Problem::SessionInvalid));
        }
        Ok(())
    }
    /// Non-500 business errors are stored too. Server errors roll back all effects and the claim.
    pub async fn finish(
        mut self,
        state: &AppState,
        response: Response,
    ) -> Result<Response, ApiError> {
        if response.status().is_server_error() {
            self.tx.rollback().await?;
            return Ok(response);
        }
        let (parts, body) = response.into_parts();
        let body = to_bytes(body, RESPONSE_LIMIT)
            .await
            .map_err(|_| ApiError::internal("idempotency_response_body"))?;
        let headers = parts
            .headers
            .iter()
            .filter(|(k, _)| HEADERS.contains(&k.as_str()))
            .map(|(k, v)| {
                Ok((
                    k.to_string(),
                    v.to_str()
                        .map_err(|_| ApiError::internal("idempotency_invalid_stored_response"))?
                        .to_owned(),
                ))
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        let sealed = state.idem.seal(&self.aad, &headers, &body);
        idempotency::complete(&mut self.tx, self.claim_id, parts.status.as_u16(), &sealed).await?;
        self.tx.commit().await?;
        let PostCommit { revocations, hooks } = *self.after_commit;
        for (id, fingerprints) in revocations {
            crate::revocation::publish(state, id, fingerprints);
        }
        if !parts.status.is_success() || hooks.is_empty() {
            return Ok(Response::from_parts(parts, Body::from(body)));
        }
        let mut refreshed = Response::from_parts(parts.clone(), Body::from(body.clone()));
        for hook in hooks {
            refreshed = hook(state.clone(), refreshed).await;
        }
        // Never turn successful committed effects into a 500 if refreshing the response fails.
        // The provisional sealed success remains replayable, with conservative pending flags.
        let (new_parts, new_body) = refreshed.into_parts();
        if new_parts.status != parts.status {
            tracing::warn!("after-commit hook changed status; retaining sealed success");
            return Ok(Response::from_parts(parts, Body::from(body)));
        }
        let updated = async {
            let bytes = to_bytes(new_body, RESPONSE_LIMIT)
                .await
                .map_err(|_| ApiError::internal("after_commit_body"))?;
            let headers = new_parts
                .headers
                .iter()
                .filter(|(k, _)| HEADERS.contains(&k.as_str()))
                .map(|(k, v)| {
                    Ok((
                        k.to_string(),
                        v.to_str()
                            .map_err(|_| ApiError::internal("after_commit_headers"))?
                            .to_owned(),
                    ))
                })
                .collect::<Result<Vec<_>, ApiError>>()?;
            let sealed = state.idem.seal(&self.aad, &headers, &bytes);
            let mut tx = porchlight_db::tx::begin(&state.pool).await?;
            idempotency::complete(&mut tx, self.claim_id, new_parts.status.as_u16(), &sealed)
                .await?;
            tx.commit().await?;
            Ok::<_, ApiError>(Response::from_parts(new_parts, Body::from(bytes)))
        }
        .await;
        match updated {
            Ok(response) => Ok(response),
            Err(_) => {
                tracing::warn!("after-commit response refresh failed; retaining sealed success");
                Ok(Response::from_parts(parts, Body::from(body)))
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    #[test]
    fn sealed_envelope_and_mac() {
        let keys = IdempotencyKeys::new(&[1; 32]);
        let other = IdempotencyKeys::new(&[2; 32]);
        let headers = vec![
            ("content-type".into(), "application/json".into()),
            ("set-cookie".into(), "secret".into()),
            ("location".into(), "/v1/me".into()),
            ("etag".into(), "version".into()),
        ];
        let a = keys.seal("device:id:key", &headers, b"response");
        let b = keys.seal("device:id:key", &headers, b"response");
        assert_ne!(a, b);
        let (h, body) = keys
            .open("device:id:key", &a)
            .unwrap_or_else(|_| panic!("valid envelope"));
        assert_eq!(h.len(), 3);
        assert_eq!(body, b"response");
        let binary: Vec<u8> = (0..=255).cycle().take(RESPONSE_LIMIT).collect();
        let sealed = keys.seal("binary", &[], &binary);
        assert!(
            sealed.len() <= binary.len() + 64,
            "body stays bytes, not a JSON number array"
        );
        assert_eq!(
            keys.open("binary", &sealed)
                .unwrap_or_else(|_| panic!("binary envelope"))
                .1,
            binary
        );
        assert!(keys.open("device:other:key", &a).is_err());
        assert!(other.open("device:id:key", &a).is_err());
        let mut corrupt = a;
        corrupt[12] ^= 1;
        assert!(keys.open("device:id:key", &corrupt).is_err());
        assert_ne!(
            keys.request_mac("POST", "/v1?x=1", b"body"),
            keys.request_mac("POST", "/v1?x=2", b"body")
        );
    }
    #[cfg(feature = "testing")]
    #[sqlx::test(migrations = "../db/migrations")]
    async fn rollback_on_internal_error(pool: sqlx::PgPool) {
        let s = crate::testing::TestServer::start(&pool).await;
        let actor = Actor {
            member_id: Some(s.init.admin_member_id),
            device_id: None,
            request_id: Uuid::now_v7(),
            agent_label: None,
            token_id: None,
        };
        let scope = IdemScope {
            kind: ScopeKind::Enrollment,
            scope_id: Uuid::now_v7(),
            household_id: s.init.household_id,
        };
        let Begin::Fresh(mut tx) = begin(
            &s.state,
            &actor,
            &crate::extract::MutationAuth::Enrollment,
            scope,
            "rollback",
            &[0; 32],
        )
        .await
        .unwrap_or_else(|_| panic!("claim")) else {
            panic!("fresh")
        };
        sqlx::query("UPDATE porchlight.user_credentials SET failed_attempts=3")
            .execute(&mut *tx.tx)
            .await
            .unwrap();
        let r = tx
            .finish(
                &s.state,
                ApiError::internal("idempotency_invalid_stored_response").into_response(),
            )
            .await
            .unwrap_or_else(|_| panic!("rollback"));
        assert_eq!(r.status(), 500);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.idempotency_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        let failures: i32 =
            sqlx::query_scalar("SELECT failed_attempts FROM porchlight.user_credentials")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(failures, 0);
        s.stop().await;
    }
    #[cfg(feature = "testing")]
    #[sqlx::test(migrations = "../db/migrations")]
    async fn hook_cannot_change_committed_status(pool: sqlx::PgPool) {
        let s = crate::testing::TestServer::start(&pool).await;
        let actor = Actor {
            member_id: Some(s.init.admin_member_id),
            device_id: None,
            request_id: Uuid::now_v7(),
            agent_label: None,
            token_id: None,
        };
        let scope_id = Uuid::now_v7();
        let scope = || IdemScope {
            kind: ScopeKind::Enrollment,
            scope_id,
            household_id: s.init.household_id,
        };
        let Begin::Fresh(mut tx) = begin(
            &s.state,
            &actor,
            &crate::extract::MutationAuth::Enrollment,
            scope(),
            "hook-status",
            &[0; 32],
        )
        .await
        .unwrap_or_else(|_| panic!("claim")) else {
            panic!("fresh")
        };
        tx.after_commit(Box::new(|_, _| {
            Box::pin(async { StatusCode::BAD_REQUEST.into_response() })
        }));
        let response = tx
            .finish(&s.state, (StatusCode::CREATED, "original").into_response())
            .await
            .unwrap_or_else(|_| panic!("finish"));
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "original"
        );
        let Begin::Replay(response) = begin(
            &s.state,
            &actor,
            &crate::extract::MutationAuth::Enrollment,
            scope(),
            "hook-status",
            &[0; 32],
        )
        .await
        .unwrap_or_else(|_| panic!("replay")) else {
            panic!("replay")
        };
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "original"
        );
        s.stop().await;
    }
}
