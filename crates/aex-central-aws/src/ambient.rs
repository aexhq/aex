//! The three ambient sources a domain is forbidden to read for itself.
//!
//! Each is a one-line adapter over a real source, and each exists so the
//! application crates can state their dependency in a signature instead of
//! reaching for a global. The tests here assert the properties the application
//! relies on — millisecond truncation, time ordering, and that the random
//! source is neither constant nor silently degraded.

use aex_identity_app::ports::{Clock, IdFactory};
use aex_identity_domain::SecretRng;
use time::OffsetDateTime;
use uuid::Uuid;

/// The wall clock, truncated to whole milliseconds.
///
/// Truncation happens here rather than at each call site because every stored
/// instant is a `timestamptz` the adapter binds as epoch milliseconds: a clock
/// with finer resolution than the store produces a value that does not survive
/// its own round trip, and a comparison that straddles the boundary decides
/// differently before and after a write.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        let raw = OffsetDateTime::now_utc();
        let millis = raw.unix_timestamp_nanos().div_euclid(1_000_000);
        OffsetDateTime::from_unix_timestamp_nanos(millis * 1_000_000).unwrap_or(raw)
    }
}

/// The identifier factory: `UUIDv7`, which every AEX row id is.
///
/// Time-ordered rather than random because keyset paging orders by
/// `(created_at, id)` and the wire's `Uuid7` refuses any other version, so a
/// `v4` here is a `500` at the edge rather than a subtle ordering bug.
#[derive(Debug, Default, Clone, Copy)]
pub struct Uuid7Factory;

impl IdFactory for Uuid7Factory {
    fn next(&self) -> Uuid {
        Uuid::now_v7()
    }
}

/// The system cryptographic random source.
///
/// [`SecretRng::fill`] returns no error, so a failing source has exactly two
/// possible behaviours and one of them is minting a credential from bytes the
/// generator did not produce. This one aborts instead: a process that cannot
/// produce randomness cannot mint credentials, and saying so loudly is the only
/// safe answer.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsSecretRng;

impl SecretRng for OsSecretRng {
    fn fill(&self, out: &mut [u8]) {
        use aws_lc_rs::rand::SecureRandom as _;
        aws_lc_rs::rand::SystemRandom::new()
            .fill(out)
            .unwrap_or_else(|_| {
                panic!("the system random source refused; no credential may be minted without it")
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{OsSecretRng, SystemClock, Uuid7Factory};
    use aex_identity_app::ports::{Clock as _, IdFactory as _};
    use aex_identity_domain::SecretRng as _;

    #[test]
    fn the_clock_is_truncated_to_whole_milliseconds() {
        let now = SystemClock.now();
        assert_eq!(
            now.unix_timestamp_nanos() % 1_000_000,
            0,
            "a sub-millisecond instant does not survive a `timestamptz` round trip"
        );
    }

    #[test]
    fn the_clock_does_not_run_backwards_across_two_reads() {
        let first = SystemClock.now();
        let second = SystemClock.now();
        assert!(second >= first, "{second} preceded {first}");
    }

    #[test]
    fn every_identifier_is_a_distinct_version_seven() {
        let ids: Vec<_> = (0..64).map(|_| Uuid7Factory.next()).collect();
        for id in &ids {
            assert_eq!(id.get_version_num(), 7, "{id} is not a UUIDv7");
        }
        let distinct: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(distinct.len(), ids.len(), "the factory repeated itself");
    }

    #[test]
    fn identifiers_minted_in_order_sort_in_order() {
        let first = Uuid7Factory.next();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = Uuid7Factory.next();
        assert!(second > first, "{second:?} did not sort after {first:?}");
    }

    #[test]
    fn the_random_source_fills_every_byte_it_is_given() {
        // A generator that writes nothing leaves the buffer at its sentinel, and
        // a credential minted from that sentinel is guessable. Two draws over a
        // 32-byte buffer collide with probability 2^-256.
        let mut first = [0xAA_u8; 32];
        let mut second = [0xAA_u8; 32];
        OsSecretRng.fill(&mut first);
        OsSecretRng.fill(&mut second);
        assert_ne!(first, [0xAA_u8; 32], "the buffer was left untouched");
        assert_ne!(
            first, [0_u8; 32],
            "the buffer was zeroed rather than filled"
        );
        assert_ne!(first, second, "the source repeated itself");
    }

    #[test]
    fn an_empty_request_is_answered_without_a_panic() {
        let mut nothing: [u8; 0] = [];
        OsSecretRng.fill(&mut nothing);
    }
}
