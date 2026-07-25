//! Response identity: `id`, `created`, `request_id` and `system_fingerprint`.
//!
//! In the default mode every one of these is derived from the plan digest, so two
//! identical requests produce byte-identical output - including the fields that
//! would otherwise make snapshots and cross-machine comparisons useless. The
//! wall-clock mode exists for demos where a realistic `created` matters more.

use std::time::{SystemTime, UNIX_EPOCH};

/// Where `created` comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdentityMode {
    /// Everything derives from the request digest. Reproducible.
    #[default]
    Derived,
    /// `created` follows the wall clock; ids still derive from the digest.
    Clock,
}

/// Base instant for derived timestamps: 2025-01-01T00:00:00Z.
const DERIVED_EPOCH: i64 = 1_735_689_600;
const DERIVED_WINDOW: u64 = 31_536_000;

/// A resolved identity for one completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub id: String,
    pub created: i64,
    pub request_id: String,
    pub system_fingerprint: String,
}

impl Identity {
    /// Derives an identity from a plan digest.
    ///
    /// `fp_` plus ten hex digits of the plan digest is decision D5: OpenAI's shape,
    /// plan-derived content, so a changed backend plan is observable.
    pub fn derive(digest: u64, mode: IdentityMode, clock: &dyn Clock) -> Self {
        let secondary = mix(digest);
        Self {
            id: format!("chatcmpl-{digest:016x}{:08x}", secondary as u32),
            created: match mode {
                IdentityMode::Derived => DERIVED_EPOCH + (digest % DERIVED_WINDOW) as i64,
                IdentityMode::Clock => clock.unix_timestamp(),
            },
            request_id: format!("req_{digest:016x}{secondary:016x}"),
            system_fingerprint: format!("fp_{:010x}", digest & 0xff_ffff_ffff),
        }
    }
}

/// A second independent value from one digest, so `id` and `request_id` differ.
fn mix(digest: u64) -> u64 {
    let mut value = digest ^ 0x9e37_79b9_7f4a_7c15;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value.wrapping_mul(0x94d0_49bb_1331_11eb)
}

/// The clock, injectable so tests never depend on wall time.
pub trait Clock: Send + Sync {
    fn unix_timestamp(&self) -> i64;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_timestamp(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }
}

/// A clock pinned to one instant.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn unix_timestamp(&self) -> i64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_identity_is_reproducible_and_shaped_like_the_real_api() {
        let first = Identity::derive(0x0123_4567_89ab_cdef, IdentityMode::Derived, &SystemClock);
        let second = Identity::derive(0x0123_4567_89ab_cdef, IdentityMode::Derived, &SystemClock);
        assert_eq!(first, second);
        assert!(first.id.starts_with("chatcmpl-"));
        assert_eq!(first.id.len(), "chatcmpl-".len() + 24);
        assert!(first.request_id.starts_with("req_"));
        assert_eq!(first.system_fingerprint.len(), 3 + 10);
        assert!(first.system_fingerprint.starts_with("fp_"));
        assert!(first.created >= DERIVED_EPOCH);
    }

    #[test]
    fn a_different_digest_changes_every_member() {
        let a = Identity::derive(1, IdentityMode::Derived, &SystemClock);
        let b = Identity::derive(2, IdentityMode::Derived, &SystemClock);
        assert_ne!(a.id, b.id);
        assert_ne!(a.request_id, b.request_id);
        assert_ne!(a.system_fingerprint, b.system_fingerprint);
    }

    #[test]
    fn id_and_request_id_are_not_the_same_derivation() {
        let identity = Identity::derive(42, IdentityMode::Derived, &SystemClock);
        assert!(
            !identity
                .request_id
                .contains(&identity.id["chatcmpl-".len()..])
        );
    }

    #[test]
    fn clock_mode_uses_the_injected_clock() {
        let identity = Identity::derive(7, IdentityMode::Clock, &FixedClock(1_700_000_000));
        assert_eq!(identity.created, 1_700_000_000);
    }
}
