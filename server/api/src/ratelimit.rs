//! Per-process token buckets. Both applicable buckets must have a token before either is charged.
use crate::error::ApiError;
use std::{collections::HashMap, net::IpAddr, sync::Mutex, time::Instant};
use uuid::Uuid;
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum Class {
    Auth,
    Write,
}
#[derive(Clone, Copy, Debug)]
pub struct RateLimits {
    pub auth_device: u32,
    pub auth_addr: u32,
    pub write_device: u32,
    pub write_addr: u32,
}
impl RateLimits {
    pub fn production() -> Self {
        Self {
            auth_device: 5,
            auth_addr: 20,
            write_device: 60,
            write_addr: 300,
        }
    }
    pub fn generous() -> Self {
        Self {
            auth_device: 1000,
            auth_addr: 1000,
            write_device: 1000,
            write_addr: 1000,
        }
    }
}
impl Default for RateLimits {
    fn default() -> Self {
        Self::production()
    }
}
#[derive(Clone, Copy, Hash, Eq, PartialEq)]
enum Key {
    Device(Uuid),
    Address(IpAddr),
}
struct Bucket {
    tokens: f64,
    last: Instant,
}
pub struct RateLimiter {
    limits: RateLimits,
    buckets: Mutex<HashMap<(Class, Key), Bucket>>,
    clock: std::sync::Arc<dyn Fn() -> Instant + Send + Sync>,
}
impl RateLimiter {
    pub fn new(limits: RateLimits) -> Self {
        Self::with_clock(limits, std::sync::Arc::new(Instant::now))
    }
    /// Inject a monotonic clock for deterministic bucket exhaustion/refill tests.
    pub fn with_clock(
        limits: RateLimits,
        clock: std::sync::Arc<dyn Fn() -> Instant + Send + Sync>,
    ) -> Self {
        assert!(
            limits.auth_device > 0
                && limits.auth_addr > 0
                && limits.write_device > 0
                && limits.write_addr > 0
        );
        Self {
            limits,
            buckets: Mutex::new(HashMap::new()),
            clock,
        }
    }
    /// Agent callers pass neither key; they are unlimited.
    pub fn check(
        &self,
        class: Class,
        device: Option<Uuid>,
        addr: Option<IpAddr>,
    ) -> Result<(), ApiError> {
        let now = (self.clock)();
        let mut buckets = self
            .buckets
            .lock()
            .map_err(|_| ApiError::internal("rate_limiter_poisoned"))?;
        // A full minute refills every bucket; forgotten idle buckets are equivalent to full ones.
        buckets.retain(|_, b| now.duration_since(b.last).as_secs() < 120);
        let (d, a) = match class {
            Class::Auth => (self.limits.auth_device, self.limits.auth_addr),
            Class::Write => (self.limits.write_device, self.limits.write_addr),
        };
        let keys = [
            device.map(|id| (Key::Device(id), d)),
            addr.map(|ip| (Key::Address(ip), a)),
        ];
        let mut retry = 0u64;
        for (key, capacity) in keys.iter().flatten() {
            let b = buckets.entry((class, *key)).or_insert(Bucket {
                tokens: f64::from(*capacity),
                last: now,
            });
            let speed = f64::from(*capacity) / 60.0;
            b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * speed)
                .min(f64::from(*capacity));
            b.last = now;
            if b.tokens < 1.0 {
                retry = retry.max(((1.0 - b.tokens) / speed).ceil() as u64);
            }
        }
        if retry > 0 {
            return Err(ApiError::new(crate::error::Problem::RateLimited).retry(retry));
        }
        for (key, _) in keys.iter().flatten() {
            buckets
                .get_mut(&(class, *key))
                .expect("bucket inserted")
                .tokens -= 1.0;
        }
        Ok(())
    }
}
impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RateLimits::production())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn production_and_classes() {
        let r = RateLimiter::default();
        let d = Uuid::now_v7();
        for _ in 0..5 {
            assert!(r.check(Class::Auth, Some(d), None).is_ok());
        }
        let e = r.check(Class::Auth, Some(d), None).err().unwrap();
        assert_eq!(e.slug(), "rate-limited");
        assert!(e.retry_after().unwrap() >= 1);
        assert!(r.check(Class::Write, Some(d), None).is_ok());
        for _ in 0..1000 {
            assert!(r.check(Class::Auth, None, None).is_ok());
        }
    }
}
