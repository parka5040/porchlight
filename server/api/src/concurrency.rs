//! Active-body quotas. Idle identities occupy no entries; disconnect drops the permit.
use crate::error::{ApiError, Problem};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Default)]
struct Counts {
    devices: HashMap<Uuid, usize>,
    members: HashMap<Uuid, usize>,
}
pub struct Limits {
    counts: Arc<Mutex<Counts>>,
    device_cap: usize,
    member_cap: Option<usize>,
}
pub struct Permit {
    counts: Arc<Mutex<Counts>>,
    device: Uuid,
    member: Option<Uuid>,
}
impl Limits {
    pub fn new(device_cap: usize, member_cap: Option<usize>) -> Self {
        Self {
            counts: Arc::default(),
            device_cap,
            member_cap,
        }
    }
    pub fn acquire(&self, device: Uuid, member: Uuid) -> Result<Permit, ApiError> {
        let mut counts = self
            .counts
            .lock()
            .map_err(|_| ApiError::internal("quota_poisoned"))?;
        if counts.devices.get(&device).copied().unwrap_or(0) >= self.device_cap
            || self
                .member_cap
                .is_some_and(|cap| counts.members.get(&member).copied().unwrap_or(0) >= cap)
        {
            return Err(ApiError::new(Problem::RateLimited).retry(1));
        }
        *counts.devices.entry(device).or_default() += 1;
        let member = self.member_cap.map(|_| {
            *counts.members.entry(member).or_default() += 1;
            member
        });
        Ok(Permit {
            counts: self.counts.clone(),
            device,
            member,
        })
    }
}
/// FIFO member admission; weak entries do not retain idle identities.
pub struct MemberSlots {
    slots: Mutex<HashMap<Uuid, std::sync::Weak<tokio::sync::Semaphore>>>,
    cap: usize,
}
impl MemberSlots {
    pub fn new(cap: usize) -> Self {
        Self {
            slots: Mutex::default(),
            cap,
        }
    }
    pub async fn acquire(
        &self,
        member: Uuid,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
        let semaphore = {
            let mut slots = self
                .slots
                .lock()
                .map_err(|_| ApiError::internal("quota_poisoned"))?;
            slots.retain(|_, slot| slot.strong_count() > 0);
            if let Some(semaphore) = slots.get(&member).and_then(std::sync::Weak::upgrade) {
                semaphore
            } else {
                let semaphore = Arc::new(tokio::sync::Semaphore::new(self.cap));
                slots.insert(member, Arc::downgrade(&semaphore));
                semaphore
            }
        };
        semaphore
            .acquire_owned()
            .await
            .map_err(|_| ApiError::internal("quota_closed"))
    }
}
pub struct SnapshotPermit {
    pub device: Permit,
    pub member: tokio::sync::OwnedSemaphorePermit,
    pub global: tokio::sync::OwnedSemaphorePermit,
}
/// A body-transforming layer can upgrade this weak handle before consuming the
/// original body, then transfer admission to its replacement. Response metadata
/// alone never extends a permit's lifetime.
#[derive(Clone)]
pub(crate) struct BodyGuard(pub std::sync::Weak<dyn Send + Sync>);

/// Keep quota ownership in the body, including while Hyper is not polling it.
/// EOF, a stream error, cancellation and disconnect all release admission.
pub fn with_body<P: Send + Sync + 'static>(
    response: axum::response::Response,
    permit: P,
) -> axum::response::Response {
    use futures_util::StreamExt;
    let (mut parts, body) = response.into_parts();
    let permit = Arc::new(permit);
    let erased: Arc<dyn Send + Sync> = permit.clone();
    parts.extensions.insert(BodyGuard(Arc::downgrade(&erased)));
    let stream = futures_util::stream::unfold(
        (body.into_data_stream(), Some(permit)),
        |(mut stream, permit)| async move {
            permit.as_ref()?;
            let chunk = stream.next().await?;
            let (chunk, permit) = match chunk {
                Ok(bytes) => {
                    // Hyper can retain a data frame after dropping the body at EOF.
                    // The bytes also own admission until the network releases them.
                    let owner = GuardedBytes {
                        bytes,
                        _permit: permit.as_ref().unwrap().clone(),
                    };
                    (Ok(axum::body::Bytes::from_owner(owner)), permit)
                }
                Err(error) => (Err(error), None),
            };
            Some((chunk, (stream, permit)))
        },
    );
    axum::response::Response::from_parts(parts, axum::body::Body::from_stream(stream))
}
struct GuardedBytes<P> {
    bytes: axum::body::Bytes,
    _permit: Arc<P>,
}
impl<P> AsRef<[u8]> for GuardedBytes<P> {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        fn release(counts: &mut HashMap<Uuid, usize>, id: Uuid) {
            let count = counts.get_mut(&id).expect("active quota entry");
            *count -= 1;
            if *count == 0 {
                counts.remove(&id);
            }
        }
        release(&mut counts.devices, self.device);
        if let Some(member) = self.member {
            release(&mut counts.members, member);
        }
    }
}
