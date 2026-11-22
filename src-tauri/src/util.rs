// Trovul - Shared helpers: poison-tolerant locks, clocks, randomness, HMAC.

use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{
    LazyLock, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// A panic on one thread must not take every later lock user down with it, so
// all global state goes through these instead of `.lock().unwrap()`.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn read<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(PoisonError::into_inner)
}

pub fn write<T>(l: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(PoisonError::into_inner)
}

/// Wall-clock milliseconds since the Unix epoch. Only for display/persistence —
/// use `mono_ms` for anything that measures elapsed time.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Monotonic milliseconds since process start, immune to clock adjustments.
/// Never returns 0, so 0 can be used as "never" in atomics.
pub fn mono_ms() -> u64 {
    START.elapsed().as_millis() as u64 + 1
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    if getrandom::fill(&mut buf).is_err() {
        // The OS RNG does not fail on supported Windows versions; degrade
        // instead of panicking if it somehow does.
        log::error!("OS random number generator unavailable, using a time-based fallback");
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (seed >> ((i % 16) * 8)) as u8 ^ (i as u8).wrapping_mul(151);
        }
    }
    buf
}

pub fn random_hex<const N: usize>() -> String {
    random_bytes::<N>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn random_nonzero_u32() -> u32 {
    loop {
        let v = u32::from_le_bytes(random_bytes::<4>());
        if v != 0 {
            return v;
        }
    }
}

/// Lock-free f32 cell for publishing measurements from real-time threads.
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub const fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }

    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block_key = [0u8; BLOCK];
    if key.len() > BLOCK {
        block_key[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; BLOCK];
    let mut outer_pad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        inner_pad[i] ^= block_key[i];
        outer_pad[i] ^= block_key[i];
    }
    let inner = Sha256::new()
        .chain_update(inner_pad)
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(outer_pad)
        .chain_update(inner)
        .finalize()
        .into()
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Comparison whose duration doesn't depend on where the inputs differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        // Test case 1
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            to_hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // Test case 6: key longer than the block size
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            to_hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn constant_time_comparison() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
