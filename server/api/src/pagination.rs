//! Keyset pagination with opaque, context-bound HMAC-signed JSON key tuples.
use crate::error::{ApiError, Problem};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Serialize, de::DeserializeOwned};
use sha2::Sha256;
use zeroize::Zeroizing;
#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageParams {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
impl PageParams {
    pub fn limit(&self) -> Result<usize, ApiError> {
        match self.limit.unwrap_or(50) {
            n @ 1..=200 => Ok(n as usize),
            _ => Err(ApiError::new(Problem::InvalidRequest)),
        }
    }
}
#[derive(Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}
pub struct CursorKeys(Zeroizing<[u8; 32]>);
impl CursorKeys {
    pub fn new(master: &[u8; 32]) -> Self {
        let mut key = Zeroizing::new([0; 32]);
        hkdf::Hkdf::<Sha256>::new(Some(b""), master)
            .expand(b"porchlight/cursor/mac", &mut *key)
            .expect("valid HKDF length");
        Self(key)
    }
    fn mac(&self, context: &str, payload: &[u8]) -> Hmac<Sha256> {
        let mut mac =
            <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(&*self.0).expect("HMAC key length");
        mac.update(context.as_bytes());
        mac.update(b"\n");
        mac.update(payload);
        mac
    }
    pub fn encode<T: Serialize>(&self, context: &str, tuple: &T) -> Result<String, ApiError> {
        let mut bytes =
            serde_json::to_vec(tuple).map_err(|_| ApiError::internal("cursor_serialization"))?;
        let tag = self.mac(context, &bytes).finalize().into_bytes();
        bytes.extend_from_slice(&tag);
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }
    pub fn decode<T: DeserializeOwned>(&self, context: &str, cursor: &str) -> Result<T, ApiError> {
        let invalid = || ApiError::new(Problem::InvalidRequest);
        if cursor.len() > 4096 {
            return Err(invalid());
        }
        let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
        if bytes.len() < 32 {
            return Err(invalid());
        }
        let (payload, tag) = bytes.split_at(bytes.len() - 32);
        self.mac(context, payload)
            .verify_slice(tag)
            .map_err(|_| invalid())?;
        serde_json::from_slice(payload).map_err(|_| invalid())
    }
    pub fn page<T: Serialize, K: Serialize>(
        &self,
        context: &str,
        mut rows: Vec<T>,
        limit: usize,
        key: impl Fn(&T) -> K,
    ) -> Result<Page<T>, ApiError> {
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next_cursor = if has_more {
            rows.last()
                .map(|r| self.encode(context, &key(r)))
                .transpose()?
        } else {
            None
        };
        Ok(Page {
            items: rows,
            next_cursor,
        })
    }
}
