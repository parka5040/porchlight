//! Per-process token buckets. Both applicable buckets must have a token before either is charged.
use crate::error::ApiError;
use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, Ipv6Addr},
    sync::Mutex,
    time::Instant,
};
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
impl Bucket {
    fn available(&self, now: Instant, capacity: u32) -> f64 {
        (self.tokens + now.duration_since(self.last).as_secs_f64() * f64::from(capacity) / 60.0)
            .min(f64::from(capacity))
    }
}
const MAX_BUCKETS: usize = 4096;
const SWEEP_BATCH: usize = 16;
#[derive(Default)]
struct ClassBuckets {
    // Reserve half the class budget for each admission domain. Anonymous enrollment
    // can never consume space or address tokens reserved for authenticated devices.
    anonymous: Buckets,
    authenticated: Buckets,
}
#[derive(Default)]
struct Buckets {
    entries: HashMap<Key, Bucket>,
    sweep: VecDeque<Key>,
}
impl Buckets {
    fn expire(&mut self, now: Instant) {
        // Each key appears once in this ring; work is bounded even during address churn.
        for _ in 0..SWEEP_BATCH.min(self.sweep.len()) {
            let key = self.sweep.pop_front().expect("sweep entry");
            if now.duration_since(self.entries[&key].last).as_secs() >= 120 {
                self.entries.remove(&key);
            } else {
                self.sweep.push_back(key);
            }
        }
        // Geometric shrinking amortizes reallocation; never scan the map per request.
        if self.entries.capacity() > 64 && self.entries.len() * 4 < self.entries.capacity() {
            self.entries.shrink_to(self.entries.len().max(16));
            self.sweep.shrink_to(self.sweep.len().max(16));
        }
    }
}
fn address(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(ip) => IpAddr::V6(Ipv6Addr::from(u128::from(ip) & (!0u128 << 64))),
        ip => ip,
    }
}
pub struct RateLimiter {
    limits: RateLimits,
    buckets: Mutex<[ClassBuckets; 2]>,
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
            buckets: Mutex::default(),
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
        let (buckets, d, a) = match class {
            Class::Auth => (
                &mut buckets[0],
                self.limits.auth_device,
                self.limits.auth_addr,
            ),
            Class::Write => (
                &mut buckets[1],
                self.limits.write_device,
                self.limits.write_addr,
            ),
        };
        let buckets = if device.is_some() {
            &mut buckets.authenticated
        } else {
            &mut buckets.anonymous
        };
        buckets.expire(now);
        let keys = [
            device.map(|id| (Key::Device(id), d)),
            addr.map(|ip| (Key::Address(address(ip)), a)),
        ];
        let mut retry = 0u64;
        let mut missing = 0;
        // Project refill without changing state: a rejection must neither admit keys
        // nor extend any existing bucket's lifetime (including an uncharged peer).
        for (key, capacity) in keys.iter().flatten() {
            if let Some(b) = buckets.entries.get(key) {
                let speed = f64::from(*capacity) / 60.0;
                let available = b.available(now, *capacity);
                if available < 1.0 {
                    retry = retry.max(((1.0 - available) / speed).ceil() as u64);
                }
            } else {
                missing += 1;
            }
        }
        if buckets.entries.len() + missing > MAX_BUCKETS / 2 {
            retry = retry.max(1);
        }
        if retry > 0 {
            return Err(ApiError::new(crate::error::Problem::RateLimited).retry(retry));
        }
        for (key, capacity) in keys.iter().flatten() {
            let b = buckets.entries.entry(*key).or_insert_with(|| {
                buckets.sweep.push_back(*key);
                Bucket {
                    tokens: f64::from(*capacity),
                    last: now,
                }
            });
            b.tokens = b.available(now, *capacity) - 1.0;
            b.last = now;
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
    fn storage(r: &RateLimiter, class: Class) -> (usize, usize) {
        let buckets = r.buckets.lock().unwrap();
        let buckets = &buckets[match class {
            Class::Auth => 0,
            Class::Write => 1,
        }];
        (
            buckets.anonymous.entries.len() + buckets.authenticated.entries.len(),
            buckets.anonymous.entries.capacity() + buckets.authenticated.entries.capacity(),
        )
    }
    #[test]
    fn audit_trust_address_churn_is_bounded_and_reclaimed() {
        let now = Instant::now();
        let clock = std::sync::Arc::new(Mutex::new(now));
        let r = RateLimiter::with_clock(RateLimits::production(), {
            let clock = clock.clone();
            std::sync::Arc::new(move || *clock.lock().unwrap())
        });
        for n in 0..20000u32 {
            let ip = std::net::Ipv6Addr::new(0x2001, 0xdb8, (n >> 16) as u16, n as u16, 0, 0, 0, 1);
            let _ = r.check(Class::Auth, None, Some(ip.into()));
        }
        let (len, capacity) = storage(&r, Class::Auth);
        assert!(len <= 4096, "address table exceeded 4096 buckets: {len}");
        assert!(
            capacity <= 8192,
            "address allocation exceeded bounded budget"
        );
        // Idle entries are removed in bounded sweep batches, not a full-map hot-path scan.
        *clock.lock().unwrap() = now + std::time::Duration::from_secs(121);
        for _ in 0..1024 {
            let _ = r.check(Class::Auth, None, None);
        }
        let (len, capacity) = storage(&r, Class::Auth);
        assert_eq!(len, 0);
        assert!(capacity <= 64, "idle table must shrink: {capacity}");
    }
    #[test]
    fn audit_trust_ipv6_prefix_and_exhausted_device() {
        let r = RateLimiter::default();
        for n in 0..20u16 {
            assert!(
                r.check(
                    Class::Auth,
                    None,
                    Some(std::net::Ipv6Addr::new(0x2001, 0xdb8, 1, 2, 0, 0, 0, n).into())
                )
                .is_ok()
            );
        }
        assert!(
            r.check(
                Class::Auth,
                None,
                Some("2001:db8:1:2::ffff".parse().unwrap())
            )
            .is_err(),
            "IPv6 addresses in one /64 share a bucket"
        );
        assert!(
            r.check(Class::Auth, None, Some("2001:db8:1:3::1".parse().unwrap()))
                .is_ok()
        );
        let d = Uuid::now_v7();
        for _ in 0..5 {
            assert!(r.check(Class::Auth, Some(d), None).is_ok());
        }
        let before = storage(&r, Class::Auth).0;
        for n in 0..20000u32 {
            assert!(
                r.check(
                    Class::Auth,
                    Some(d),
                    Some(std::net::Ipv4Addr::from(n).into())
                )
                .is_err()
            );
        }
        assert_eq!(
            storage(&r, Class::Auth).0,
            before,
            "exhausted devices cannot admit addresses"
        );
    }
    #[test]
    fn audit_trust_rejected_checks_do_not_refresh_idle_lifetime() {
        let now = Instant::now();
        let clock = std::sync::Arc::new(Mutex::new(now));
        let r = RateLimiter::with_clock(
            RateLimits {
                auth_addr: 1,
                ..RateLimits::production()
            },
            {
                let clock = clock.clone();
                std::sync::Arc::new(move || *clock.lock().unwrap())
            },
        );
        let addr = Some("192.168.77.1".parse().unwrap());
        assert!(r.check(Class::Auth, None, addr).is_ok());
        for second in 1..60 {
            *clock.lock().unwrap() = now + std::time::Duration::from_secs(second);
            assert!(r.check(Class::Auth, None, addr).is_err());
        }
        *clock.lock().unwrap() = now + std::time::Duration::from_secs(121);
        assert!(r.check(Class::Auth, None, None).is_ok());
        assert_eq!(
            storage(&r, Class::Auth).0,
            0,
            "rejected checks must not refresh an entry's idle lifetime"
        );
    }
    #[test]
    fn audit_trust_partitioned_admission_remains_bounded_and_reclaims() {
        let now = Instant::now();
        let clock = std::sync::Arc::new(Mutex::new(now));
        let r = RateLimiter::with_clock(RateLimits::production(), {
            let clock = clock.clone();
            std::sync::Arc::new(move || *clock.lock().unwrap())
        });
        for class in [Class::Auth, Class::Write] {
            for n in 0..20000u16 {
                let addr = Some(Ipv6Addr::new(0x2001, 0xdb8, n, 0, 0, 0, 0, 1).into());
                let _ = r.check(class, None, addr);
                let _ = r.check(class, Some(Uuid::from_u128(u128::from(n) + 1)), addr);
            }
            let (len, capacity) = storage(&r, class);
            assert_eq!(len, MAX_BUCKETS, "both reserved partitions must be usable");
            assert!(
                capacity <= 8192,
                "aggregate allocation budget must not increase"
            );
        }
        for tables in r.buckets.lock().unwrap().iter() {
            for table in [&tables.anonymous, &tables.authenticated] {
                assert_eq!(table.entries.len(), MAX_BUCKETS / 2);
                assert_eq!(table.sweep.len(), table.entries.len());
                assert!(table.sweep.capacity() <= MAX_BUCKETS / 2);
            }
        }
        *clock.lock().unwrap() = now + std::time::Duration::from_secs(121);
        for class in [Class::Auth, Class::Write] {
            for _ in 0..1024 {
                assert!(r.check(class, None, None).is_ok());
                let _ = r.check(class, Some(Uuid::nil()), None);
            }
            let (len, capacity) = storage(&r, class);
            assert_eq!(len, 1, "only the sweep-driving device should remain");
            assert!(capacity <= 128, "both idle partitions must shrink");
        }
    }
    #[test]
    fn audit_trust_rejected_pair_does_not_refresh_either_bucket() {
        let now = Instant::now();
        let clock = std::sync::Arc::new(Mutex::new(now));
        let r = RateLimiter::with_clock(RateLimits::production(), {
            let clock = clock.clone();
            std::sync::Arc::new(move || *clock.lock().unwrap())
        });
        let device = Some(Uuid::now_v7());
        let addr = Some("192.168.77.1".parse().unwrap());
        for _ in 0..5 {
            assert!(r.check(Class::Auth, device, addr).is_ok());
        }
        *clock.lock().unwrap() = now + std::time::Duration::from_secs(1);
        assert!(r.check(Class::Auth, device, addr).is_err());
        let buckets = r.buckets.lock().unwrap();
        assert!(
            buckets[0]
                .authenticated
                .entries
                .values()
                .all(|b| b.last == now),
            "a rejected pair must not refresh even its nonexhausted address bucket"
        );
    }
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
