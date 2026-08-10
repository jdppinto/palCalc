//! Rate limiting: a per-IP token bucket plus a global aggregate bucket.
//!
//! Dependency-free and small enough to audit.
//!
//! * Per-IP: each source IP gets a bucket refilling at `per_second` up to
//!   `burst`; a request costs one token. The bucket map is HARD-capped at
//!   `MAX_TRACKED_IPS`: when full and a new IP arrives, the oldest ~10% of
//!   entries are evicted in one pass (amortized O(1) per request), so a flood
//!   of distinct source IPs can neither grow memory without bound nor make
//!   each request an O(n) scan.
//! * Global: a single bucket caps aggregate request rate across *all* IPs, so
//!   a rotated/distributed flood (which defeats any per-IP scheme) is still
//!   bounded at the HTTP layer. (Connection/TLS-handshake floods are a
//!   network-layer concern — front the port with a firewall/fail2ban.)

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

/// Hard ceiling on distinct tracked IPs. At capacity, the oldest slice is
/// evicted so memory stays bounded regardless of source-IP cardinality.
const MAX_TRACKED_IPS: usize = 10_000;
/// Fraction (1/N) of entries dropped per eviction pass, amortizing the O(n)
/// scan across many inserts.
const EVICT_FRACTION: usize = 10;

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    fn take(&mut self, now: Instant, per_second: f64, burst: f64) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * per_second).min(burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

struct Inner {
    per_ip: HashMap<IpAddr, Bucket>,
    global: Bucket,
}

pub struct RateLimiter {
    per_second: f64,
    burst: f64,
    global_per_second: f64,
    global_burst: f64,
    inner: Mutex<Inner>,
}

impl RateLimiter {
    pub fn new(per_second: u64, burst: u32) -> RateLimiter {
        let per_second = per_second.max(1) as f64;
        let burst = burst.max(1) as f64;
        // Aggregate cap: generous for a handful of friends, but bounds a
        // distributed flood. Derived from the per-IP knobs.
        let global_per_second = (per_second * 20.0).max(50.0);
        let global_burst = (burst * 10.0).max(200.0);
        RateLimiter {
            per_second,
            burst,
            global_per_second,
            global_burst,
            inner: Mutex::new(Inner {
                per_ip: HashMap::new(),
                global: Bucket {
                    tokens: global_burst,
                    last: Instant::now(),
                },
            }),
        }
    }

    /// Consume one token for `ip`. Returns true if allowed.
    pub fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        // Recover from poisoning rather than panicking every future request:
        // the state is a best-effort counter, so a stale view is acceptable.
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        // Global aggregate cap first — this is what bounds rotated floods.
        if !inner.global.take(now, self.global_per_second, self.global_burst) {
            return false;
        }

        // Enforce the hard size cap before inserting a new IP.
        if inner.per_ip.len() >= MAX_TRACKED_IPS && !inner.per_ip.contains_key(&ip) {
            evict_oldest_slice(&mut inner.per_ip);
        }

        let (per_second, burst) = (self.per_second, self.burst);
        inner
            .per_ip
            .entry(ip)
            .or_insert(Bucket {
                tokens: burst,
                last: now,
            })
            .take(now, per_second, burst)
    }
}

/// Drop the oldest ~1/EVICT_FRACTION of entries in a single O(n) pass. Called
/// only when the map is full, so it runs at most once per ~MAX/EVICT_FRACTION
/// inserts — amortized O(1) per request.
fn evict_oldest_slice(map: &mut HashMap<IpAddr, Bucket>) {
    let mut times: Vec<Instant> = map.values().map(|b| b.last).collect();
    if times.is_empty() {
        return;
    }
    let cut = (times.len() / EVICT_FRACTION).max(1).min(times.len() - 1);
    times.select_nth_unstable(cut);
    let cutoff = times[cut];
    // Keep entries strictly newer than the cutoff.
    map.retain(|_, b| b.last > cutoff);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_limit() {
        let rl = RateLimiter::new(1, 3);
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert!(rl.allow(ip));
        assert!(rl.allow(ip));
        assert!(rl.allow(ip));
        assert!(!rl.allow(ip)); // 4th within the same instant is limited
    }

    #[test]
    fn separate_ips_independent() {
        let rl = RateLimiter::new(1, 1);
        let a: IpAddr = "1.1.1.1".parse().unwrap();
        let b: IpAddr = "2.2.2.2".parse().unwrap();
        assert!(rl.allow(a));
        assert!(rl.allow(b)); // b has its own bucket
        assert!(!rl.allow(a));
    }

    #[test]
    fn map_stays_bounded_under_distinct_ip_flood() {
        // burst=2000 → global_burst=20000, enough to admit all 15000 distinct
        // IPs so the per-IP eviction path (not the global cap) is exercised.
        let rl = RateLimiter::new(1000, 2000);
        for i in 0..(MAX_TRACKED_IPS as u32 + 5000) {
            let ip = IpAddr::from(std::net::Ipv4Addr::from(i));
            let _ = rl.allow(ip);
        }
        let inner = rl.inner.lock().unwrap();
        assert!(
            inner.per_ip.len() <= MAX_TRACKED_IPS,
            "map exceeded hard cap: {}",
            inner.per_ip.len()
        );
    }
}
