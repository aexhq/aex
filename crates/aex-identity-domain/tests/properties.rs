//! Generated properties over the identity state machines and codecs.

use aex_control_domain::Revision;
use aex_identity_domain::challenge::{ChallengeState, EMAIL_CHALLENGE_TTL, EmailChallenge};
use aex_identity_domain::credential::{
    CredentialKind, PepperVersion, RegionCode, SecretRng, WorkspacePin, decode_id, encode_id, mint,
    parse, verifier, verify,
};
use aex_identity_domain::session::{DASHBOARD_SESSION_TTL, DashboardSession, SessionState};
use aex_identity_domain::user::{User, UserStatus};
use aex_identity_domain::{NormalizedEmail, Pepper};
use proptest::prelude::*;
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
        let pin = kind.carries_workspace_pin().then(|| WorkspacePin {
            region: RegionCode::ALL[4],
            workspace: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0002),
        });
        let rng = Recorded::new(seed);
        let (secret, digest) = mint(kind, pin, id, &rng);
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
        let pin = kind.carries_workspace_pin().then(|| WorkspacePin {
            region: RegionCode::ALL[4],
            workspace: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0002),
        });
        let (secret, digest) = mint(kind, pin, id, &Recorded::new(vec![4]));
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
