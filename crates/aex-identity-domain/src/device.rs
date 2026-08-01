//! The device authorization grant.
//!
//! RFC 8628 shaped and AEX-authoritative: `Pending → {Approved → Consumed,
//! Denied, Expired}`, TTL 10 minutes, poll interval 5 seconds.
//!
//! The user code is `XXXXX-XXXXX` over a 20-symbol unambiguous alphabet, which
//! is about 43.2 bits, minted by **rejection sampling**. The system this
//! replaces used `XXXX-XXXX` over 31 symbols — about 35 bits — and picked each
//! symbol with `byte % 31`, which is modulo-biased: nine of the thirty-one
//! symbols were 33% more likely than the rest. This fixes both.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use aex_control_domain::scope::ScopeSet;

use crate::credential::{PepperVersion, SecretRng};

/// How long a device grant lives.
pub const DEVICE_TTL: Duration = Duration::minutes(10);
/// How often a device may poll.
pub const DEVICE_POLL_INTERVAL: Duration = Duration::seconds(5);
/// How much a `slow_down` raises the interval.
pub const DEVICE_SLOW_DOWN_STEP: Duration = Duration::seconds(5);
/// The longest interval a `slow_down` may raise the poll to.
pub const DEVICE_MAX_POLL_INTERVAL: Duration = Duration::seconds(60);

/// The unambiguous user-code alphabet: consonants only, no vowels and no
/// digit-lookalikes, so a code cannot be misread and cannot spell a word.
pub const USER_CODE_ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";
/// How many symbols each half of a user code holds.
pub const USER_CODE_HALF: usize = 5;

/// Where a device grant is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceState {
    /// Waiting for a person to approve.
    Pending,
    /// Approved, not yet redeemed.
    Approved,
    /// Refused by a person.
    Denied,
    /// Redeemed into an account token.
    Consumed,
    /// Lapsed without being redeemed.
    Expired,
}

impl DeviceState {
    /// Every state.
    pub const ALL: [Self; 5] = [
        Self::Pending,
        Self::Approved,
        Self::Denied,
        Self::Consumed,
        Self::Expired,
    ];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Consumed => "consumed",
            Self::Expired => "expired",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }

    /// Whether the state can never change again.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Denied | Self::Consumed | Self::Expired)
    }
}

/// Why a user code was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UserCodeError {
    /// The wrong number of symbols.
    #[error("a user code is {} symbols", USER_CODE_HALF * 2)]
    Length,
    /// A symbol outside the alphabet.
    #[error("a user code symbol is outside the alphabet")]
    Symbol,
}

/// A human-typed device code, `XXXXX-XXXXX`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserCode(String);

impl UserCode {
    /// Mints a code by rejection sampling.
    ///
    /// Rejection sampling rather than a modulo: with 20 symbols, `byte % 20`
    /// would make the first sixteen symbols 1.25 times more likely than the last
    /// four, costing about a third of a bit per symbol.
    #[must_use]
    pub fn mint(rng: &dyn SecretRng) -> Self {
        let alphabet = u8::try_from(USER_CODE_ALPHABET.len()).unwrap_or(20);
        let ceiling = 256_u16 - (256_u16 % u16::from(alphabet));
        let mut symbols = Vec::with_capacity(USER_CODE_HALF * 2 + 1);
        let mut buffer = [0_u8; 32];
        let mut cursor = buffer.len();
        while symbols.len() < USER_CODE_HALF * 2 + 1 {
            if symbols.len() == USER_CODE_HALF {
                symbols.push(b'-');
                continue;
            }
            if cursor == buffer.len() {
                rng.fill(&mut buffer);
                cursor = 0;
            }
            let byte = buffer[cursor];
            cursor += 1;
            if u16::from(byte) >= ceiling {
                continue;
            }
            symbols.push(USER_CODE_ALPHABET[usize::from(byte % alphabet)]);
        }
        Self(String::from_utf8(symbols).unwrap_or_default())
    }

    /// Normalizes a typed code: uppercase, hyphens and spaces stripped.
    ///
    /// # Errors
    ///
    /// Returns [`UserCodeError`] for the wrong length or a symbol outside the
    /// alphabet.
    pub fn normalize(raw: &str) -> Result<Self, UserCodeError> {
        let stripped: Vec<u8> = raw
            .bytes()
            .filter(|byte| !matches!(byte, b'-' | b' '))
            .map(|byte| byte.to_ascii_uppercase())
            .collect();
        if stripped.len() != USER_CODE_HALF * 2 {
            return Err(UserCodeError::Length);
        }
        if !stripped
            .iter()
            .all(|byte| USER_CODE_ALPHABET.contains(byte))
        {
            return Err(UserCodeError::Symbol);
        }
        let mut text = String::with_capacity(USER_CODE_HALF * 2 + 1);
        for (index, byte) in stripped.iter().enumerate() {
            if index == USER_CODE_HALF {
                text.push('-');
            }
            text.push(char::from(*byte));
        }
        Ok(Self(text))
    }

    /// The code as a string slice, hyphenated.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A device authorization grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthorization {
    /// The grant's own id, which is the device-code lookup key.
    pub id: Uuid,
    /// Where it is.
    pub state: DeviceState,
    /// What the device asked for.
    pub requested_scopes: ScopeSet,
    /// Which pepper version the verifiers were computed under.
    pub pepper_version: PepperVersion,
    /// Who approved it.
    pub approved_by: Option<Uuid>,
    /// When it was approved.
    pub approved_at: Option<OffsetDateTime>,
    /// When it was redeemed.
    pub consumed_at: Option<OffsetDateTime>,
    /// The token it minted.
    pub account_token_id: Option<Uuid>,
    /// When it was created.
    pub issued_at: OffsetDateTime,
    /// When it lapses.
    pub expires_at: OffsetDateTime,
    /// How often the device may poll.
    pub poll_interval: Duration,
    /// When it last polled.
    pub last_polled_at: Option<OffsetDateTime>,
}

/// What a poll is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollDecision {
    /// Nobody has decided yet.
    Pending,
    /// Polling too fast; the stored interval has been raised.
    SlowDown {
        /// The new interval to obey.
        interval: Duration,
    },
    /// Approved and redeemable.
    Ready,
    /// A person refused.
    Denied,
    /// It lapsed.
    Expired,
    /// It was already redeemed.
    AlreadyConsumed,
}

/// Why a device transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeviceTransition {
    /// The grant already reached a terminal state.
    #[error("the device authorization is already resolved")]
    AlreadyResolved,
    /// The grant lapsed.
    #[error("the device authorization expired")]
    Expired,
    /// The grant has not been approved.
    #[error("the device authorization has not been approved")]
    NotApproved,
    /// The approver is not a current dashboard actor.
    #[error("only an active person with an active session may approve")]
    ApproverNotCurrent,
}

impl DeviceAuthorization {
    /// Where the grant is at `now`, deriving expiry.
    #[must_use]
    pub fn state_at(&self, now: OffsetDateTime) -> DeviceState {
        if self.state.is_terminal() {
            return self.state;
        }
        if self.expires_at <= now {
            return DeviceState::Expired;
        }
        self.state
    }

    /// Approves the grant.
    ///
    /// `approver_is_current` is what the caller read in the same transaction:
    /// an `Active` person holding an `Active` session. It is a parameter rather
    /// than a lookup because this crate reads nothing.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceTransition`] for a resolved or lapsed grant, and
    /// [`DeviceTransition::ApproverNotCurrent`] otherwise.
    pub fn approve(
        &self,
        actor: Uuid,
        approver_is_current: bool,
        now: OffsetDateTime,
    ) -> Result<Self, DeviceTransition> {
        self.check_open(now)?;
        if !approver_is_current {
            return Err(DeviceTransition::ApproverNotCurrent);
        }
        Ok(Self {
            state: DeviceState::Approved,
            approved_by: Some(actor),
            approved_at: Some(now),
            ..self.clone()
        })
    }

    /// Refuses the grant.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceTransition`] for a resolved or lapsed grant.
    pub fn deny(&self, now: OffsetDateTime) -> Result<Self, DeviceTransition> {
        self.check_open(now)?;
        Ok(Self {
            state: DeviceState::Denied,
            ..self.clone()
        })
    }

    /// Redeems the grant into an account token.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceTransition::NotApproved`] for a pending grant,
    /// [`DeviceTransition::AlreadyResolved`] for a resolved one, and
    /// [`DeviceTransition::Expired`] for a lapsed one. An approved-but-unredeemed
    /// grant expires without minting anything.
    pub fn consume(&self, token_id: Uuid, now: OffsetDateTime) -> Result<Self, DeviceTransition> {
        match self.state_at(now) {
            DeviceState::Approved => Ok(Self {
                state: DeviceState::Consumed,
                account_token_id: Some(token_id),
                consumed_at: Some(now),
                ..self.clone()
            }),
            DeviceState::Pending => Err(DeviceTransition::NotApproved),
            DeviceState::Expired => Err(DeviceTransition::Expired),
            DeviceState::Denied | DeviceState::Consumed => Err(DeviceTransition::AlreadyResolved),
        }
    }

    /// Answers a poll and, when the device is polling too fast, raises the
    /// stored interval.
    #[must_use]
    pub fn poll(&self, now: OffsetDateTime) -> (Self, PollDecision) {
        let state = self.state_at(now);
        let too_fast = self
            .last_polled_at
            .is_some_and(|last| now - last < self.poll_interval);
        if too_fast && !state.is_terminal() {
            let interval =
                (self.poll_interval + DEVICE_SLOW_DOWN_STEP).min(DEVICE_MAX_POLL_INTERVAL);
            return (
                Self {
                    poll_interval: interval,
                    last_polled_at: Some(now),
                    ..self.clone()
                },
                PollDecision::SlowDown { interval },
            );
        }
        let decision = match state {
            DeviceState::Pending => PollDecision::Pending,
            DeviceState::Approved => PollDecision::Ready,
            DeviceState::Denied => PollDecision::Denied,
            DeviceState::Consumed => PollDecision::AlreadyConsumed,
            DeviceState::Expired => PollDecision::Expired,
        };
        (
            Self {
                last_polled_at: Some(now),
                ..self.clone()
            },
            decision,
        )
    }

    /// Fails unless the grant is still open.
    fn check_open(&self, now: OffsetDateTime) -> Result<(), DeviceTransition> {
        match self.state_at(now) {
            DeviceState::Pending | DeviceState::Approved => Ok(()),
            DeviceState::Expired => Err(DeviceTransition::Expired),
            DeviceState::Denied | DeviceState::Consumed => Err(DeviceTransition::AlreadyResolved),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEVICE_MAX_POLL_INTERVAL, DEVICE_POLL_INTERVAL, DEVICE_SLOW_DOWN_STEP, DEVICE_TTL,
        DeviceAuthorization, DeviceState, DeviceTransition, PollDecision, USER_CODE_ALPHABET,
        UserCode, UserCodeError,
    };
    use crate::credential::{PepperVersion, SecretRng};
    use aex_control_domain::scope::ScopeSet;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    /// A counting generator, so rejection sampling is observable.
    struct Counting(std::sync::atomic::AtomicU8);

    impl SecretRng for Counting {
        fn fill(&self, out: &mut [u8]) {
            for slot in out.iter_mut() {
                *slot = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// A generator whose first fill is entirely above the rejection ceiling.
    struct Rejected;

    impl SecretRng for Rejected {
        fn fill(&self, out: &mut [u8]) {
            static SECOND: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if SECOND.swap(true, std::sync::atomic::Ordering::Relaxed) {
                out.fill(0);
            } else {
                out.fill(250);
            }
        }
    }

    fn grant(state: DeviceState) -> DeviceAuthorization {
        DeviceAuthorization {
            id: Uuid::from_u128(1),
            state,
            requested_scopes: ScopeSet::MEMBER,
            pepper_version: PepperVersion::new(1),
            approved_by: None,
            approved_at: None,
            consumed_at: None,
            account_token_id: None,
            issued_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH + DEVICE_TTL,
            poll_interval: DEVICE_POLL_INTERVAL,
            last_polled_at: None,
        }
    }

    #[test]
    fn the_alphabet_is_twenty_unambiguous_symbols() {
        assert_eq!(USER_CODE_ALPHABET.len(), 20);
        let mut sorted = USER_CODE_ALPHABET.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 20, "no symbol is listed twice");
        for vowel in b"AEIOU" {
            assert!(!USER_CODE_ALPHABET.contains(vowel), "{vowel} is a vowel");
        }
        for lookalike in b"0O1I" {
            assert!(!USER_CODE_ALPHABET.contains(lookalike));
        }
    }

    #[test]
    fn a_minted_code_has_the_expected_shape() {
        let code = UserCode::mint(&Counting(std::sync::atomic::AtomicU8::new(0)));
        assert_eq!(code.as_str().len(), 11);
        assert_eq!(code.as_str().as_bytes()[5], b'-');
        assert!(
            code.as_str()
                .bytes()
                .filter(|byte| *byte != b'-')
                .all(|byte| USER_CODE_ALPHABET.contains(&byte))
        );
    }

    #[test]
    fn rejection_sampling_admits_no_byte_at_or_above_the_ceiling() {
        // 256 mod 20 is 16, so bytes 240..=255 must be rejected outright.
        let ceiling = 256 - (256 % USER_CODE_ALPHABET.len());
        assert_eq!(ceiling, 240);
        let code = UserCode::mint(&Rejected);
        assert!(
            code.as_str()
                .bytes()
                .filter(|byte| *byte != b'-')
                .all(|byte| byte == USER_CODE_ALPHABET[0]),
            "a fully rejected fill produces no symbol at all"
        );
    }

    #[test]
    fn normalization_accepts_every_human_spelling() {
        for raw in ["BCDFG-HJKLM", "bcdfg-hjklm", "BCDFGHJKLM", "bcd fg-hjk lm"] {
            assert_eq!(
                UserCode::normalize(raw).map(|it| it.as_str().to_owned()),
                Ok("BCDFG-HJKLM".to_owned()),
                "{raw}"
            );
        }
        assert_eq!(UserCode::normalize("BCDFG"), Err(UserCodeError::Length));
        assert_eq!(
            UserCode::normalize("AEIOU-BCDFG"),
            Err(UserCodeError::Symbol)
        );
    }

    #[test]
    fn approval_requires_a_current_dashboard_actor() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::minutes(1);
        assert_eq!(
            grant(DeviceState::Pending).approve(Uuid::from_u128(9), false, now),
            Err(DeviceTransition::ApproverNotCurrent)
        );
        let approved = grant(DeviceState::Pending)
            .approve(Uuid::from_u128(9), true, now)
            .expect("a current actor approves");
        assert_eq!(approved.state, DeviceState::Approved);
        assert_eq!(approved.approved_by, Some(Uuid::from_u128(9)));
        assert_eq!(approved.approved_at, Some(now));
    }

    #[test]
    fn a_consumed_grant_always_names_its_approver() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::minutes(1);
        let consumed = grant(DeviceState::Pending)
            .approve(Uuid::from_u128(9), true, now)
            .expect("approves")
            .consume(Uuid::from_u128(11), now)
            .expect("consumes");
        assert_eq!(consumed.state, DeviceState::Consumed);
        assert_eq!(consumed.account_token_id, Some(Uuid::from_u128(11)));
        assert!(consumed.approved_by.is_some());
    }

    #[test]
    fn a_grant_is_consumed_at_most_once() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::minutes(1);
        let approved = grant(DeviceState::Approved);
        let consumed = approved.consume(Uuid::from_u128(11), now).expect("first");
        assert_eq!(
            consumed.consume(Uuid::from_u128(12), now),
            Err(DeviceTransition::AlreadyResolved)
        );
    }

    #[test]
    fn a_pending_grant_cannot_be_consumed() {
        assert_eq!(
            grant(DeviceState::Pending).consume(
                Uuid::from_u128(11),
                OffsetDateTime::UNIX_EPOCH + Duration::minutes(1)
            ),
            Err(DeviceTransition::NotApproved)
        );
    }

    #[test]
    fn an_approved_but_unconsumed_grant_expires_without_minting() {
        let after = OffsetDateTime::UNIX_EPOCH + DEVICE_TTL;
        let approved = grant(DeviceState::Approved);
        assert_eq!(approved.state_at(after), DeviceState::Expired);
        assert_eq!(
            approved.consume(Uuid::from_u128(11), after),
            Err(DeviceTransition::Expired)
        );
    }

    #[test]
    fn a_denied_grant_never_returns_to_pending() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::minutes(1);
        let denied = grant(DeviceState::Pending).deny(now).expect("denies");
        assert_eq!(denied.state, DeviceState::Denied);
        assert_eq!(
            denied.approve(Uuid::from_u128(9), true, now),
            Err(DeviceTransition::AlreadyResolved)
        );
        assert_eq!(denied.deny(now), Err(DeviceTransition::AlreadyResolved));
    }

    #[test]
    fn polling_faster_than_the_interval_raises_it() {
        let start = OffsetDateTime::UNIX_EPOCH + Duration::minutes(1);
        let (polled, decision) = grant(DeviceState::Pending).poll(start);
        assert_eq!(decision, PollDecision::Pending);
        assert_eq!(polled.last_polled_at, Some(start));

        let (faster, decision) = polled.poll(start + Duration::seconds(1));
        assert_eq!(
            decision,
            PollDecision::SlowDown {
                interval: DEVICE_POLL_INTERVAL + DEVICE_SLOW_DOWN_STEP
            }
        );
        assert_eq!(
            faster.poll_interval,
            DEVICE_POLL_INTERVAL + DEVICE_SLOW_DOWN_STEP
        );

        let (patient, decision) = faster.poll(start + Duration::seconds(30));
        assert_eq!(decision, PollDecision::Pending);
        assert_eq!(patient.poll_interval, faster.poll_interval);
    }

    #[test]
    fn the_slow_down_interval_is_bounded() {
        let mut grant = grant(DeviceState::Pending);
        grant.poll_interval = DEVICE_MAX_POLL_INTERVAL;
        grant.last_polled_at = Some(OffsetDateTime::UNIX_EPOCH);
        let (raised, decision) = grant.poll(OffsetDateTime::UNIX_EPOCH + Duration::seconds(1));
        assert_eq!(
            decision,
            PollDecision::SlowDown {
                interval: DEVICE_MAX_POLL_INTERVAL
            }
        );
        assert_eq!(raised.poll_interval, DEVICE_MAX_POLL_INTERVAL);
    }

    #[test]
    fn a_poll_reports_every_terminal_state() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::minutes(1);
        for (state, expected) in [
            (DeviceState::Approved, PollDecision::Ready),
            (DeviceState::Denied, PollDecision::Denied),
            (DeviceState::Consumed, PollDecision::AlreadyConsumed),
        ] {
            let (_, decision) = grant(state).poll(now);
            assert_eq!(decision, expected, "{state:?}");
        }
        let (_, decision) =
            grant(DeviceState::Pending).poll(OffsetDateTime::UNIX_EPOCH + DEVICE_TTL);
        assert_eq!(decision, PollDecision::Expired);
    }

    #[test]
    fn the_state_spellings_round_trip() {
        for state in DeviceState::ALL {
            assert_eq!(DeviceState::parse(state.as_str()), Some(state));
        }
        assert_eq!(DeviceState::parse("issued"), None);
    }
}
