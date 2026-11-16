//! Row-local JSON construction with bounded growth and optional incremental gzip.
use crate::error::ApiError;
use serde::Serialize;
use std::io::Write;

pub const SYNC_BUDGET: usize = 8 * 1024 * 1024;
// Eight bounded window/cursor records and all remaining delimiters/metadata.
pub const SYNC_FOOTER: usize = 32 * 1024;
const CHUNK: usize = 32 * 1024;

#[derive(Default)]
struct Buffer(Vec<u8>);
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let required = self.0.len() + bytes.len();
        if required > self.0.capacity() {
            let capacity = required
                .max(self.0.capacity() + self.0.capacity() / 8)
                .max(8192);
            self.0.reserve_exact(capacity - self.0.len());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
struct Output {
    pending: Buffer,
    encoder: Option<flate2::write::GzEncoder<Buffer>>,
    gzip: bool,
    force: bool,
    len: usize,
    limit: usize,
    failed: bool,
}
impl Output {
    fn new() -> Self {
        let (gzip, force) = crate::compression::document_coding();
        Self {
            pending: Buffer::default(),
            encoder: None,
            gzip,
            force,
            len: 0,
            limit: usize::MAX,
            failed: false,
        }
    }
    fn push(&mut self, byte: u8) {
        if self.write_all(&[byte]).is_err() {
            self.failed = true;
        }
    }
    fn error(&self) -> ApiError {
        if self.failed && self.limit != usize::MAX {
            ApiError::new(crate::error::Problem::ItemSyncTooLarge)
        } else {
            ApiError::internal("json_document")
        }
    }
    fn finish(mut self) -> Result<(Vec<u8>, bool), ApiError> {
        if self.failed {
            return Err(self.error());
        }
        if let Some(mut encoder) = self.encoder.take() {
            encoder
                .write_all(&self.pending.0)
                .map_err(|_| ApiError::internal("json_document_gzip"))?;
            Ok((
                encoder
                    .finish()
                    .map_err(|_| ApiError::internal("json_document_gzip"))?
                    .0,
                true,
            ))
        } else {
            Ok((self.pending.0, false))
        }
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.failed || bytes.len() > self.limit.saturating_sub(self.len) {
            self.failed = true;
            return Err(std::io::Error::other("JSON document budget exceeded"));
        }
        self.len += bytes.len();
        if self.gzip && self.encoder.is_none() && (self.force || self.len >= 8192) {
            self.encoder = Some(flate2::write::GzEncoder::new(
                Buffer::default(),
                flate2::Compression::new(1),
            ));
        }
        if let Some(encoder) = &mut self.encoder {
            // Never retain a section-sized identity buffer beside encoded output.
            let mut remaining = bytes;
            while !remaining.is_empty() {
                let take = remaining.len().min(CHUNK - self.pending.0.len());
                self.pending.write_all(&remaining[..take])?;
                remaining = &remaining[take..];
                if self.pending.0.len() == CHUNK {
                    encoder.write_all(&self.pending.0)?;
                    self.pending.0.clear();
                }
            }
        } else {
            self.pending.write_all(bytes)?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct Document {
    bytes: Output,
    first: bool,
}
impl Default for Document {
    fn default() -> Self {
        let mut bytes = Output::new();
        bytes.push(b'{');
        Self { bytes, first: true }
    }
}
fn key(bytes: &mut Output, first: &mut bool, name: &str) -> Result<(), ApiError> {
    if !*first {
        bytes.push(b',');
    }
    *first = false;
    serde_json::to_writer(&mut *bytes, name).map_err(|_| bytes.error())?;
    bytes.push(b':');
    Ok(())
}
impl Document {
    pub fn bounded(limit: usize) -> Self {
        let mut document = Self::default();
        document.bytes.limit = limit;
        document
    }
    pub fn field(&mut self, name: &str, value: &impl Serialize) -> Result<(), ApiError> {
        key(&mut self.bytes, &mut self.first, name)?;
        serde_json::to_writer(&mut self.bytes, value).map_err(|_| self.bytes.error())
    }
    pub fn array(&mut self, name: &str) -> Result<Array<'_>, ApiError> {
        key(&mut self.bytes, &mut self.first, name)?;
        self.bytes.push(b'[');
        Ok(Array {
            bytes: &mut self.bytes,
            first: true,
        })
    }
    pub fn identity_len(&self) -> usize {
        self.bytes.len
    }
    pub fn finish(mut self) -> Vec<u8> {
        self.bytes.push(b'}');
        self.bytes.finish().expect("unbounded JSON document").0
    }
    pub fn response(mut self) -> Result<axum::response::Response, ApiError> {
        use axum::{
            http::{HeaderValue, header},
            response::IntoResponse,
        };
        self.bytes.push(b'}');
        let (bytes, gzip) = self.bytes.finish()?;
        let mut response = ([(header::CONTENT_TYPE, "application/json")], bytes).into_response();
        if gzip {
            response
                .headers_mut()
                .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        }
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        Ok(response)
    }
}
pub struct Array<'a> {
    bytes: &'a mut Output,
    first: bool,
}
impl Array<'_> {
    fn separator(&mut self) {
        if !self.first {
            self.bytes.push(b',');
        }
        self.first = false;
    }
    /// Stage just one row to decide whether it belongs to this page or its continuation.
    pub fn try_row(&mut self, value: &impl Serialize, budget: usize) -> Result<bool, ApiError> {
        let row = serde_json::to_vec(value).map_err(|_| self.bytes.error())?;
        if self.bytes.len + row.len() + usize::from(!self.first) + SYNC_FOOTER > budget {
            return Ok(false);
        }
        self.separator();
        self.bytes.write_all(&row).map_err(|_| self.bytes.error())?;
        Ok(true)
    }
    pub fn object(&mut self) -> Object<'_> {
        self.separator();
        self.bytes.push(b'{');
        Object {
            bytes: self.bytes,
            first: true,
        }
    }
    pub fn row(&mut self, value: &impl Serialize) -> Result<(), ApiError> {
        self.separator();
        serde_json::to_writer(&mut *self.bytes, value).map_err(|_| self.bytes.error())
    }
    /// Compact trusted PostgreSQL JSON text without a Value tree.
    pub fn sql_row(&mut self, json: &str) {
        self.separator();
        let (mut quoted, mut escaped) = (false, false);
        for byte in json.bytes() {
            if quoted {
                self.bytes.push(byte);
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quoted = false;
                }
            } else if byte == b'"' {
                quoted = true;
                self.bytes.push(byte);
            } else if !byte.is_ascii_whitespace() {
                self.bytes.push(byte);
            }
        }
    }
    pub fn finish(self) {
        self.bytes.push(b']');
    }
}
pub struct Object<'a> {
    bytes: &'a mut Output,
    first: bool,
}
impl Object<'_> {
    pub fn field(&mut self, name: &str, value: &impl Serialize) -> Result<(), ApiError> {
        key(self.bytes, &mut self.first, name)?;
        serde_json::to_writer(&mut *self.bytes, value).map_err(|_| self.bytes.error())
    }
    pub fn array(&mut self, name: &str) -> Result<Array<'_>, ApiError> {
        key(self.bytes, &mut self.first, name)?;
        self.bytes.push(b'[');
        Ok(Array {
            bytes: self.bytes,
            first: true,
        })
    }
    pub fn finish(self) {
        self.bytes.push(b'}');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_sections_preserve_escapes_and_scalar_types() {
        let mut doc = Document::default();
        doc.field("cursor", &5).unwrap();
        let mut a = doc.array("rows").unwrap();
        a.sql_row(r#"{ "text": "a b\n\"\\", "number": 1.25, "flag": true, "nil": null }"#);
        a.row(&serde_json::json!({"unicode":"\u{2603}"})).unwrap();
        a.finish();
        let a = doc.array("empty").unwrap();
        a.finish();
        let value: serde_json::Value = serde_json::from_slice(&doc.finish()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"cursor":5,"rows":[{"text":"a b\n\"\\","number":1.25,"flag":true,"nil":null},{"unicode":"\u{2603}"}],"empty":[]})
        );
    }
}
