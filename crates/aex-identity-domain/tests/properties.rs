//! Generated properties over the identity state machines and codecs.

use aex_control_domain::Revision;
use aex_identity_domain::challenge::{ChallengeState, EMAIL_CHALLENGE_TTL, EmailChallenge};
use aex_identity_domain::credential::{
    CredentialKind, PepperVersion, RegionCode, SecretRng, decode_id, encode_id, mint, parse,
    verifier, verify,
};
use aex_identity_domain::device::{
    DEVICE_TTL, DeviceAuthorization, DeviceState, USER_CODE_ALPHABET, UserCode,
};
use aex_identity_domain::session::{DASHBOARD_SESSION_TTL, DashboardSession, SessionState};
use aex_identity_domain::user::{User, UserStatus};
use aex_identity_domain::{NormalizedEmail, Pepper};
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// A generator that replays a recorded byte stream, so a history is exactly
/// reproducible from its seed.
struct Recorded(Mutex<Vec<u8>>);

impl Recorded {
    fn new(seed: Vec<u8>) -> Self {
        Self(Mutex::new(seed))
    }
}

impl SecretRng for Recorded {
    fn fill(&self, out: &mut [u8]) {
        let mut source = self.0.lock().expect("the recorded stream");
        for slot in out.iter_mut() {
            let byte = source.pop().unwrap_or(0);
            source.insert(0, byte);
            *slot = byte;
        }
    }
}

/// One command against a session.
#[derive(Debug, Clone, Copy)]
enum SessionCommand {
    Extend(i64),
    Revoke,
    Observe(i64),
}

fn session() -> DashboardSession {
    DashboardSession {
        id: Uuid::from_u128(1),
        user_id: Uuid::from_u128(2),
        pepper_version: PepperVersion::new(1),
        issued_at: OffsetDateTime::UNIX_EPOCH,
        expires_at: OffsetDateTime::UNIX_EPOCH + DASHBOARD_SESSION_TTL,
        revoked_at: None,
    }
}

fn challenge() -> EmailChallenge {
    EmailChallenge {
        id: Uuid::from_u128(1),
        email: NormalizedEmail::parse("a@b.test").expect("valid"),
        pepper_version: PepperVersion::new(1),
        issued_at: OffsetDateTime::UNIX_EPOCH,
        expires_at: OffsetDateTime::UNIX_EPOCH + EMAIL_CHALLENGE_TTL,
        consumed_at: None,
    }
}

fn grant(state: DeviceState) -> DeviceAuthorization {
    DeviceAuthorization {
        id: Uuid::from_u128(1),
        state,
        requested_scopes: aex_control_domain::ScopeSet::MEMBER,
        pepper_version: PepperVersion::new(1),
        approved_by: None,
        approved_at: None,
        consumed_at: None,
        account_token_id: None,
        issued_at: OffsetDateTime::UNIX_EPOCH,
        expires_at: OffsetDateTime::UNIX_EPOCH + DEVICE_TTL,
        poll_interval: aex_identity_domain::DEVICE_POLL_INTERVAL,
        last_polled_at: None,
    }
}

fn user() -> User {
    User {
        id: Uuid::from_u128(1),
        email: NormalizedEmail::parse("a@b.test").expect("valid"),
        email_verified_at: None,
        name: None,
        image_url: None,
        status: UserStatus::Active,
        revision: Revision::INITIAL,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn any_kind() -> impl Strategy<Value = CredentialKind> {
    proptest::sample::select(CredentialKind::ALL.to_vec())
}

proptest! {
    /// P1: a challenge leaves its issued state at most once, whatever order the
    /// consumers arrive in.
    #[test]
    fn a_challenge_is_consumed_at_most_once(
        offsets in proptest::collection::vec(0_i64..2000, 1..12)
    ) {
        let mut current = challenge();
        let mut winners = 0;
        for offset in offsets {
            let now = OffsetDateTime::UNIX_EPOCH + Duration::seconds(offset);
            if let Ok(next) = current.consume(now) {
                winners += 1;
                current = next;
            }
        }
        prop_assert!(winners <= 1, "{winners} consumers won");
    }

    /// P2: a consumed challenge never returns to issued.
    #[test]
    fn a_consumed_challenge_never_resurrects(observe in 0_i64..100_000) {
        let consumed = challenge()
            .consume(OffsetDateTime::UNIX_EPOCH + Duration::seconds(1))
            .expect("consumes");
        let at = OffsetDateTime::UNIX_EPOCH + Duration::seconds(observe);
        prop_assert!(matches!(
            consumed.state_at(at),
            ChallengeState::Consumed | ChallengeState::Expired
        ));
        prop_assert!(consumed.consume(at).is_err());
    }

    /// P3: expiry is monotone non-increasing after issuance, and extension is
    /// capped at `issued_at + 30 d`.
    #[test]
    fn session_expiry_is_bounded_under_every_history(
        commands in proptest::collection::vec(
            prop_oneof![
                (0_i64..4_000_000).prop_map(SessionCommand::Extend),
                Just(SessionCommand::Revoke),
                (0_i64..4_000_000).prop_map(SessionCommand::Observe),
            ],
            0..24,
        )
    ) {
        let start = session();
        let mut current = start.clone();
        let mut now = OffsetDateTime::UNIX_EPOCH;
        for command in commands {
            match command {
                SessionCommand::Extend(seconds) => {
                    let target = OffsetDateTime::UNIX_EPOCH + Duration::seconds(seconds);
                    if let Ok(next) = current.extend(target, now) {
                        prop_assert!(next.expires_at <= start.expiry_cap());
                        prop_assert!(next.expires_at >= current.expires_at);
                        current = next;
                    }
                }
                SessionCommand::Revoke => {
                    if let Ok(next) = current.revoke(now) {
                        current = next;
                    }
                }
                SessionCommand::Observe(seconds) => {
                    now = OffsetDateTime::UNIX_EPOCH + Duration::seconds(seconds);
                }
            }
            prop_assert!(current.expires_at <= start.expiry_cap());
            if current.revoked_at.is_some() {
                prop_assert_eq!(
                    current.state_at(current.expires_at + Duration::days(1)),
                    SessionState::Revoked
                );
            }
        }
    }

    /// P2 for devices: a resolved grant never returns to pending or approved.
    #[test]
    fn a_resolved_device_grant_never_resurrects(observe in 0_i64..10_000) {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::seconds(1);
        for resolved in [
            grant(DeviceState::Pending).deny(now).expect("denies"),
            grant(DeviceState::Approved)
                .consume(Uuid::from_u128(9), now)
                .expect("consumes"),
        ] {
            let at = OffsetDateTime::UNIX_EPOCH + Duration::seconds(observe);
            prop_assert!(resolved.state_at(at).is_terminal());
            prop_assert!(resolved.approve(Uuid::from_u128(9), true, at).is_err());
            prop_assert!(resolved.consume(Uuid::from_u128(9), at).is_err());
        }
    }

    /// P10: a consumed grant always names an approver.
    #[test]
    fn a_consumed_grant_always_names_an_approver(seconds in 1_i64..500) {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::seconds(seconds);
        let consumed = grant(DeviceState::Pending)
            .approve(Uuid::from_u128(9), true, now)
            .and_then(|approved| approved.consume(Uuid::from_u128(11), now));
        if let Ok(consumed) = consumed {
            prop_assert!(consumed.approved_by.is_some());
            prop_assert!(consumed.account_token_id.is_some());
        }
    }

    /// P6: a disabled person can never be re-derived as authenticable without an
    /// explicit re-enable.
    #[test]
    fn a_disabled_person_stays_disabled(repeats in 0_usize..8) {
        let at = OffsetDateTime::UNIX_EPOCH;
        let mut current = user().disable(at).expect("disables");
        for _ in 0..repeats {
            prop_assert!(current.disable(at).is_err());
            prop_assert!(!current.may_authenticate());
            current = current.clone();
        }
    }

    /// The credential codec round-trips for every kind and every id.
    #[test]
    fn mint_parse_verify_round_trips(
        kind in any_kind(),
        bits in any::<u128>(),
        seed in proptest::collection::vec(any::<u8>(), 1..64),
    ) {
        let id = Uuid::from_u128(bits);
        let region = kind.carries_region().then(|| RegionCode::ALL[4]);
        let rng = Recorded::new(seed);
        let (secret, digest) = mint(kind, region, id, &rng);
        let parsed = parse(kind, secret.expose()).expect("a minted token parses");
        prop_assert_eq!(parsed.id, id);
        prop_assert_eq!(parsed.digest, digest);
        let pepper = Pepper::new([3_u8; 32]);
        prop_assert!(verify(&pepper, &digest, &verifier(&pepper, &digest)));
    }

    /// A mutated token never parses into the same credential.
    #[test]
    fn a_mutated_token_never_parses_the_same(
        kind in any_kind(),
        bits in any::<u128>(),
        index in 0_usize..77,
    ) {
        let id = Uuid::from_u128(bits);
        let region = kind.carries_region().then(|| RegionCode::ALL[4]);
        let (secret, digest) = mint(kind, region, id, &Recorded::new(vec![4]));
        let token = secret.expose().to_owned();
        let index = index % token.len();
        let mut mutated: Vec<u8> = token.clone().into_bytes();
        mutated[index] = if mutated[index] == b'A' { b'B' } else { b'A' };
        let mutated = String::from_utf8(mutated).unwrap_or(token);
        match parse(kind, &mutated) {
            Err(_) => {}
            Ok(parsed) => prop_assert!(
                parsed.id != id || parsed.digest != digest,
                "a mutated token parsed to the same credential"
            ),
        }
    }

    /// The Crockford id codec round-trips over the whole 128-bit space.
    #[test]
    fn the_id_codec_round_trips(bits in any::<u128>()) {
        let id = Uuid::from_u128(bits);
        prop_assert_eq!(decode_id(&encode_id(id)), Some(id));
    }

    /// A user code always normalizes back to itself.
    #[test]
    fn a_user_code_normalizes_to_itself(seed in proptest::collection::vec(0_u8..240, 16..64)) {
        let code = UserCode::mint(&Recorded::new(seed));
        prop_assert_eq!(
            UserCode::normalize(code.as_str()).map(|it| it.as_str().to_owned()),
            Ok(code.as_str().to_owned())
        );
        prop_assert_eq!(
            UserCode::normalize(&code.as_str().to_ascii_lowercase())
                .map(|it| it.as_str().to_owned()),
            Ok(code.as_str().to_owned())
        );
    }

    /// Email normalization is idempotent and case-insensitive.
    #[test]
    fn email_normalization_is_idempotent(
        local in "[a-zA-Z0-9._%+-]{1,32}",
        domain in "[a-zA-Z0-9-]{1,20}",
    ) {
        let raw = format!("{local}@{domain}.test");
        let once = NormalizedEmail::parse(&raw).expect("a well-formed address");
        let twice = NormalizedEmail::parse(once.as_str()).expect("still well formed");
        prop_assert_eq!(once.as_str(), twice.as_str());
        let upper = NormalizedEmail::parse(&raw.to_ascii_uppercase()).expect("upper case");
        prop_assert_eq!(once.as_str(), upper.as_str());
    }
}

/// The user-code alphabet is uniform under rejection sampling: over a full
/// sweep of the 240 accepted byte values every symbol is chosen exactly twelve
/// times, which is what modulo bias would break.
#[test]
fn the_user_code_alphabet_is_uniform_under_rejection_sampling() {
    let alphabet = u8::try_from(USER_CODE_ALPHABET.len()).expect("20 fits in a u8");
    let ceiling = 256_u16 - (256_u16 % u16::from(alphabet));
    let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
    for byte in 0..=u8::MAX {
        if u16::from(byte) >= ceiling {
            continue;
        }
        *counts
            .entry(USER_CODE_ALPHABET[usize::from(byte % alphabet)])
            .or_default() += 1;
    }
    assert_eq!(counts.len(), USER_CODE_ALPHABET.len());
    for (symbol, count) in counts {
        assert_eq!(
            count,
            12,
            "symbol {} is chosen {count} times, not 12",
            char::from(symbol)
        );
    }
}
