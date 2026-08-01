//! `aex-identity-domain` owns the pure identity state machine — people, provider
//! links, browser sessions, email challenges, device authorizations and account
//! tokens — plus the two codecs the whole platform authenticates through: the
//! credential codec and the 30-second authorization assertion.
//!
//! # Invariants
//!
//! - every transition is total, returns a typed error, and never mutates in place
//! - nothing here reads a clock, an RNG or the environment; all three arrive as
//!   parameters, so every history is reproducible
//! - a challenge, a device grant and an invitation are single-use; consuming one
//!   twice is a typed rejection, never a second effect
//! - `Debug` and `Display` on any secret, verifier, pepper or private key print
//!   `<redacted:N bytes>`, asserted by test
//! - an assertion lives at most thirty seconds, and [`assertion::verify`]
//!   re-checks the bound so an issuer bug cannot lengthen the window
//!
//! # Not this crate's job
//!
//! - storage, SQL or transactions (`aex-identity-aurora`)
//! - HTTP, cookies or `OAuth` provider transport (`central-identity-api`)
//! - key custody: Secrets Manager and KMS live in the deployables

pub mod assertion;
pub mod challenge;
pub mod credential;
pub mod device;
pub mod email;
pub mod link;
pub mod session;
pub mod token;
pub mod user;

pub use challenge::{ChallengeState, ChallengeTransition, EMAIL_CHALLENGE_TTL, EmailChallenge};
pub use credential::{
    CredentialError, CredentialKind, MintedSecret, ParsedCredential, Pepper, PepperVersion,
    PresentedDigest, RegionCode, SecretRng, Verifier, mint, parse, verifier, verify,
};
pub use device::{
    DEVICE_POLL_INTERVAL, DEVICE_SLOW_DOWN_STEP, DEVICE_TTL, DeviceAuthorization, DeviceState,
    DeviceTransition, PollDecision, UserCode, UserCodeError,
};
pub use email::{EmailError, NormalizedEmail};
pub use link::{ExternalIdentity, Provider, ProviderAccountId, UnlinkDenied};
pub use session::{DASHBOARD_SESSION_TTL, DashboardSession, SessionState, SessionTransition};
pub use token::{ACCOUNT_TOKEN_TTL, AccountToken, TokenOrigin, TokenTransition};
pub use user::{User, UserStatus, UserTransition};
