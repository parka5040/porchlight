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
