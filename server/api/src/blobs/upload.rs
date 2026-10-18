use super::*;
use futures_util::StreamExt;
use std::{
    io::{Seek, SeekFrom},
    sync::Arc,
    time::Duration,
};

pub const UPLOAD_IDLE: Duration = Duration::from_secs(30);
pub const UPLOAD_TOTAL: Duration = Duration::from_secs(10 * 60);
pub const PERMIT_WAIT: Duration = Duration::from_secs(30);

/// Production deadlines; tests shorten them without conflating idle and total limits.
#[derive(Clone, Copy)]
pub struct UploadLimits {
    pub idle: Duration,
    pub total: Duration,
    pub permit_wait: Duration,
}
impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            idle: UPLOAD_IDLE,
            total: UPLOAD_TOTAL,
            permit_wait: PERMIT_WAIT,
        }
    }
}
/// Weak entries do not retain idle device state; each active device/PAT has two temp slots.
#[derive(Default)]
pub struct UploadStreams(
    std::sync::Mutex<std::collections::HashMap<Uuid, std::sync::Weak<tokio::sync::Semaphore>>>,
);
impl UploadStreams {
    fn acquire(&self, id: Uuid) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
        let mut slots = self.0.lock().expect("upload stream slots");
        slots.retain(|_, slot| slot.strong_count() > 0);
        let semaphore = slots
            .get(&id)
            .and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| {
                let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(2));
                slots.insert(id, std::sync::Arc::downgrade(&semaphore));
                semaphore
            });
        semaphore
            .try_acquire_owned()
            .map_err(|_| ApiError::new(Problem::RateLimited).retry(1))
    }
}
/// The RAII temp owns cleanup on rejection, replay, cancellation, timeout and decode failure.
pub struct Upload {
    pub temp: Arc<UploadTemp>,
    _stream: tokio::sync::OwnedSemaphorePermit,
    _disk: Arc<DiskReservation>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    pub name: String,
    pub mime: Option<String>,
}
/// The final owner unlinks and retires the physical charge together, even on cancellation.
pub struct UploadTemp {
    file: Option<tempfile::NamedTempFile>,
    budget: DiskBudget,
}
impl UploadTemp {
    pub fn path(&self) -> &Path {
        self.file.as_ref().expect("live upload temp").path()
    }
    pub fn reopen(&self) -> std::io::Result<std::fs::File> {
        self.file.as_ref().expect("live upload temp").reopen()
    }
}
impl Drop for UploadTemp {
    fn drop(&mut self) {
        let file = self.file.take().expect("live upload temp");
        let path = file.path().to_owned();
        // Failed unlinks remain charged; GC can retry them later.
        let _ = self.budget.change(&[&path], || file.close());
    }
}
pub struct UploadMutation(pub Mutation<Upload>);
fn multipart_error(e: axum::extract::multipart::MultipartError) -> ApiError {
    ApiError::new(if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        Problem::AttachmentTooLarge
    } else {
        Problem::AttachmentInvalid
    })
}
impl FromRequest<AppState> for UploadMutation {
    type Rejection = ApiError;
    async fn from_request(req: Request, state: &AppState) -> Result<Self, ApiError> {
        let (req, auth, key, context, request_id) =
            crate::extract::mutation_metadata(req, state).await?;
        let stream_id = match &auth {
            crate::extract::MutationAuth::Device { id, .. }
            | crate::extract::MutationAuth::Pat { id, .. } => *id,
            _ => return Err(ApiError::new(Problem::Unauthenticated)),
        };
        let stream_slot = state.upload_streams.acquire(stream_id)?;
        let idle = state.upload_limits.idle;
        let method = req.method().as_str().to_owned();
        let path = context
            .uri
            .path_and_query()
            .map_or(context.uri.path(), |p| p.as_str())
            .to_owned();
        let tmp = state.settings.data_dir.join("blobs/tmp");
        tokio::fs::create_dir_all(&tmp)
            .await
            .map_err(|_| ApiError::internal("upload_tmp"))?;
        let disk = state.disk_budget.reserve(
            &tmp,
            MAX_UPLOAD as u64,
            state.settings.disk_reserve_bytes,
        )?;
        let temp = tempfile::Builder::new()
            .prefix(".")
            .suffix(".tmp")
            .tempfile_in(tmp)
            .map_err(|_| ApiError::internal("upload_tmp"))?;
        let temp = Arc::new(UploadTemp {
            file: Some(temp),
            budget: state.disk_budget.clone(),
        });
        let disk = Arc::new(disk);
        let (parts, body) = req.into_parts();
        // Idle means no incoming nonempty data, not no complete multipart field. Header trickles
        // also count as progress, but can never extend the independent ten-minute overall cap.
        let stalled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stalled.clone();
        let stream = futures_util::stream::unfold(body.into_data_stream(), move |mut stream| {
            let flag = flag.clone();
            async move {
                let next = tokio::time::timeout(idle, async {
                    loop {
                        match stream.next().await {
                            Some(Ok(b)) if b.is_empty() => continue,
                            v => return v,
                        }
                    }
                })
                .await;
                match next {
                    Ok(Some(v)) => Some((v, stream)),
                    Ok(None) => None,
                    Err(_) => {
                        flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        Some((
                            Err(axum::Error::new(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "upload idle",
                            ))),
                            stream,
                        ))
                    }
                }
            }
        });
        let req = Request::from_parts(parts, Body::from_stream(stream));
        let parse = async {
            let mut multipart = Multipart::from_request(req, state)
                .await
                .map_err(|_| ApiError::new(Problem::AttachmentInvalid))?;
            let mut field = multipart
                .next_field()
                .await
                .map_err(multipart_error)?
                .ok_or_else(|| ApiError::new(Problem::AttachmentInvalid))?;
            if field.name() != Some("file") {
                return Err(ApiError::new(Problem::AttachmentInvalid));
            }
            let name = field.file_name().unwrap_or("attachment").to_owned();
            let mime = field.content_type().map(str::to_owned);
            let mut len = 0;
            while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
                if chunk.len() > MAX_UPLOAD - len {
                    return Err(ApiError::new(Problem::AttachmentTooLarge));
                }
                len += chunk.len();
                let (temp, disk, state) = (temp.clone(), disk.clone(), state.clone());
                tokio::task::spawn_blocking(move || {
                    // The task retains the linked temp and both reservations through cancellation.
                    let _disk = disk;
                    let _quota = state
                        .disk_budget
                        .reserve_quota(chunk.len() as u64, state.settings.attachment_quota_bytes)?;
                    state
                        .disk_budget
                        .change(&[temp.path()], || {
                            let mut file = temp.reopen()?;
                            file.seek(SeekFrom::End(0))?;
                            file.write_all(&chunk)
                        })
                        .map_err(|_| ApiError::internal("upload_tmp_write"))
                })
                .await
                .map_err(|_| ApiError::internal("upload_tmp_task"))??;
            }
            drop(field);
            if multipart
                .next_field()
                .await
                .map_err(multipart_error)?
                .is_some()
            {
                return Err(ApiError::new(Problem::AttachmentInvalid));
            }
            Ok(Upload {
                temp,
                name,
                mime,
                _stream: stream_slot,
                _disk: disk,
                permit: None,
            })
        };
        let result = tokio::time::timeout(state.upload_limits.total, parse)
            .await
            .map_err(|_| ApiError::new(Problem::RequestTimeout))?;
        if stalled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ApiError::new(Problem::RequestTimeout));
        }
        let body = result?;
        // Hash the semantic name/MIME + raw file with constant memory and no ingest permit.
        let mut prefix = serde_json::to_vec(&(&body.name, &body.mime))
            .map_err(|_| ApiError::internal("upload_identity"))?;
        prefix.push(0);
        let mut file = body
            .temp
            .reopen()
            .map_err(|_| ApiError::internal("upload_tmp"))?;
        let keys = state.clone();
        let request_hash = tokio::task::spawn_blocking(move || {
            keys.idem
                .request_mac_reader(&method, &path, &prefix, &mut file)
        })
        .await
        .map_err(|_| ApiError::internal("upload_identity"))?
        .map_err(|_| ApiError::internal("upload_identity"))?;
        Ok(Self(Mutation {
            auth,
            key,
            request_hash,
            context,
            body,
            request_id,
            _body_permit: None,
        }))
    }
}
pub fn ingest_error(e: IngestError) -> ApiError {
    ApiError::new(match e {
        IngestError::TooLarge => Problem::AttachmentTooLarge,
        IngestError::UnsupportedType => Problem::AttachmentTypeUnsupported,
        IngestError::TypeMismatch => Problem::AttachmentTypeMismatch,
        IngestError::Invalid => Problem::AttachmentInvalid,
    })
}
fn contain_ingest<T>(f: impl FnOnce() -> Result<T, IngestError>) -> Result<T, ApiError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|_| ApiError::new(Problem::AttachmentInvalid))?
        .map_err(ingest_error)
}
/// Acquire before the mutation opens IdemTx: queued decoders never pin a pool connection.
pub async fn admit_ingest(state: &AppState, upload: &mut Upload) -> Result<(), ApiError> {
    upload.permit = Some(
        tokio::time::timeout(
            state.upload_limits.permit_wait,
            state.ingest_slots.clone().acquire_owned(),
        )
        .await
        .map_err(|_| ApiError::transient_conflict().retry(1))?
        .map_err(|_| ApiError::internal("ingest_slot"))?,
    );
    Ok(())
}
pub async fn safe_ingest(
    _state: &AppState,
    mut upload: Upload,
) -> Result<(Ingested, String), ApiError> {
    let permit = upload
        .permit
        .take()
        .ok_or_else(|| ApiError::internal("ingest_not_admitted"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let upload = upload; // Retain cleanup/reservations, not just the fields read below.
        let bytes =
            std::fs::read(upload.temp.path()).map_err(|_| ApiError::internal("upload_tmp_read"))?;
        let r = contain_ingest(|| {
            ingest::ingest(
                &bytes,
                Some(&upload.name),
                upload.mime.as_deref(),
                &Limits::production(),
            )
        })?;
        let name = sanitize_name(&upload.name, r.content_type);
        Ok((r, name))
    })
    .await
    .map_err(|_| ApiError::new(Problem::AttachmentInvalid))?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoder_panic_is_attachment_invalid() {
        assert_eq!(
            contain_ingest::<()>(|| panic!("decoder fixture"))
                .unwrap_err()
                .problem(),
            Problem::AttachmentInvalid
        );
    }
}
