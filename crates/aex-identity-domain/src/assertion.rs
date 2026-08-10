//! The 30-second authorization assertion.
//!
//! A fixed-layout 323-byte binary envelope signed Ed25519. Not JWT and not
//! JSON: there is no algorithm negotiation, no header parsing, no
//! variable-length field and no allocation, so there is no confusion or
//! expansion surface either. A decoder that reads a length other than 323, an
//! unknown discriminant, a reserved byte that is not zero, a duplicate epoch
//! subject, or an empty slot before a non-empty one fails closed.
//!
//! ```text
//! off sz field                        body offsets (235 bytes)
//!   0  4 magic = "AEXA"                 0  8 issued_at_ms
//!   4  1 version = 0x01                 8  8 expires_at_ms
//!   5  1 alg = 0x01 (Ed25519)          16  1 audience_plane
//!   6 16 kid (raw UUID)                17  1 audience_region
//!  22  2 body_len = 235                18  1 audience_service
//!  24 235 body                          19  1 principal_kind
//! 259 64 signature over bytes[0..259]   20 16 principal_id
//!                                       36 32 credential_binding
//!                                       68 16 organization_id
//!                                       84 16 workspace_id
//!                                      100  1 workspace_region
//!                                      101  1 account_state
//!                                      102  8 scopes
//!                                      110 125 epoch_subjects (5 x 25)
//! ```
//!
//! [`verify`] re-checks `expires_at_ms - issued_at_ms <= 30_000` and refuses a
//! validly signed assertion that claims longer, so an issuer bug cannot lengthen
//! the window. The signature is checked before every semantic check except
//! length, magic, version, algorithm and `kid`, so an unauthenticated caller
//! learns nothing about state.
//!
//! This module is the only dependency the regional plane takes on this crate. It
//! is pure and links no AWS SDK: the private key is unwrapped by the deployable
//! and handed in through [`AssertionSigner`].

use aex_control_domain::epoch::{Epoch, EpochSubjectKind};
use aex_control_domain::scope::{ScopeError, ScopeSet};
use aex_internal_contracts::assertion::{AssertionAudience, AssertionError, IssuedAssertion};
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::credential::PresentedDigest;

/// The longest an assertion may live.
pub const ASSERTION_MAX_LIFETIME_MS: u64 = 30_000;
/// The exact envelope length.
pub const ASSERTION_ENVELOPE_LEN: usize = 323;
/// The exact body length.
pub const ASSERTION_BODY_LEN: usize = 235;
/// How many bytes the signature covers.
pub const ASSERTION_SIGNED_LEN: usize = 259;
/// The envelope magic.
pub const ASSERTION_MAGIC: [u8; 4] = *b"AEXA";
/// The only envelope version.
pub const ASSERTION_VERSION: u8 = 1;
/// The only signature algorithm.
pub const ASSERTION_ALG_ED25519: u8 = 1;
/// How many epoch slots the envelope carries.
pub const EPOCH_SLOTS: usize = 5;
/// How many bytes one epoch slot occupies.
const EPOCH_SLOT_LEN: usize = 1 + 16 + 8;
/// The declared body length, as the two big-endian bytes the header carries.
const BODY_LEN_BE: [u8; 2] = 235_u16.to_be_bytes();
/// The most verification keys a key set may hold.
pub const MAX_VERIFICATION_KEYS: usize = 8;

const _: () = assert!(ASSERTION_ENVELOPE_LEN == ASSERTION_SIGNED_LEN + 64);
const _: () = assert!(ASSERTION_SIGNED_LEN == 24 + ASSERTION_BODY_LEN);
const _: () = assert!(ASSERTION_BODY_LEN == 110 + EPOCH_SLOTS * EPOCH_SLOT_LEN);

/// Which deployment plane an assertion is valid in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Plane {
    /// The development plane.
    Dev = 1,
    /// The production plane.
    Prd = 2,
}

impl Plane {
    /// Resolves a wire discriminant.
    #[must_use]
    pub const fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Dev),
            2 => Some(Self::Prd),
            _ => None,
        }
    }

    /// The configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prd => "prd",
        }
    }

    /// Resolves a configuration spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "dev" => Some(Self::Dev),
            "prd" => Some(Self::Prd),
            _ => None,
        }
    }
}

/// The `audience_service` byte for one audience.
///
/// The audience vocabulary itself is
/// [`aex_internal_contracts::assertion::AssertionAudience`], which is also what
/// the two `central-authz` requests carry. This function is the *codec*, and it
/// lives here because every other byte of the layout does. Declaration order in
/// that enum is wire order, so `AssertionAudience::ALL[n]` is byte `n + 1`.
#[must_use]
pub const fn audience_code(audience: AssertionAudience) -> u8 {
    match audience {
        AssertionAudience::RegionalSession => 1,
        AssertionAudience::RegionalSecret => 2,
        AssertionAudience::RegionalObservation => 3,
        AssertionAudience::RegionalOtlp => 4,
        AssertionAudience::RegionalStream => 5,
        AssertionAudience::ToolExec => 6,
    }
}

/// Resolves an `audience_service` byte.
#[must_use]
pub const fn audience_from_code(byte: u8) -> Option<AssertionAudience> {
    match byte {
        1 => Some(AssertionAudience::RegionalSession),
        2 => Some(AssertionAudience::RegionalSecret),
        3 => Some(AssertionAudience::RegionalObservation),
        4 => Some(AssertionAudience::RegionalOtlp),
        5 => Some(AssertionAudience::RegionalStream),
        6 => Some(AssertionAudience::ToolExec),
        _ => None,
    }
}

/// Which kind of principal an assertion speaks for.
///
/// `UserSession` exists so a regional dashboard panel is implementable: a
/// browser session resolves through the *same* 30-second assertion as an account
/// token rather than a second credential mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum PrincipalKind {
    /// A workspace API key.
    WorkspaceKey = 1,
    /// A person, through an account token.
    AccountActor = 2,
    /// A person, through a browser session.
    UserSession = 3,
    /// An agent session, through an activation the Brain already owns.
    ///
    /// The one kind with **no presented credential behind it**. The other three
    /// name something a customer handed over and `central-authz` checked; this
    /// one names a session `brain-mux` is already executing under a
    /// `FenceGuard`, and its envelope is signed locally rather than minted
    /// centrally.
    ///
    /// It exists as its own kind rather than reusing `WorkspaceKey` because the
    /// alternative was to let the tool executor accept an envelope whose
    /// principal slot could also have been filled by a credential a customer
    /// presented. Two things that are verified differently must not be spelled
    /// the same.
    AgentSession = 4,
}

impl PrincipalKind {
    /// Every kind, in wire order.
    pub const ALL: [Self; 4] = [
        Self::WorkspaceKey,
        Self::AccountActor,
        Self::UserSession,
        Self::AgentSession,
    ];

    /// Resolves a wire discriminant.
    #[must_use]
    pub const fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::WorkspaceKey),
            2 => Some(Self::AccountActor),
            3 => Some(Self::UserSession),
            4 => Some(Self::AgentSession),
            _ => None,
        }
    }

    /// The telemetry spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceKey => "workspace_key",
            Self::AccountActor => "account_actor",
            Self::UserSession => "user_session",
            Self::AgentSession => "agent_session",
        }
    }

    /// Whether a customer presents a credential to obtain this principal.
    ///
    /// The pairing rule the executor enforces: a presented-credential principal
    /// never reaches [`AssertionAudience::ToolExec`], and
    /// [`Self::AgentSession`] never reaches any other audience.
    #[must_use]
    pub const fn is_presented_credential(self) -> bool {
        match self {
            Self::WorkspaceKey | Self::AccountActor | Self::UserSession => true,
            Self::AgentSession => false,
        }
    }
}

/// The account state an assertion carries.
///
/// There is no `unavailable` arm: an assertion is never issued over an unreadable
/// account state, which is what makes `503 account_state_unavailable` a refusal
/// rather than an optimistic guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum AssertedAccountState {
    /// Admission proceeds.
    Active = 1,
    /// The account owes a top-up; only pause-exempt routes run.
    PausedTopUpRequired = 2,
}

impl AssertedAccountState {
    /// Resolves a wire discriminant.
    #[must_use]
    pub const fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Active),
            2 => Some(Self::PausedTopUpRequired),
            _ => None,
        }
    }
}

/// Who an assertion is addressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Audience {
    /// Which plane.
    pub plane: Plane,
    /// Which region.
    pub region: aex_wire::types::Region,
    /// Which service.
    pub service: AssertionAudience,
}

/// One `(kind, id, epoch)` slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EpochSlot {
    /// Which subject kind, or [`EpochSubjectKind::Empty`] for an unused slot.
    pub kind: EpochSubjectKind,
    /// Which subject.
    pub id: Uuid,
    /// Its epoch at issue time.
    pub epoch: Epoch,
}

impl EpochSlot {
    /// An unused slot.
    pub const EMPTY: Self = Self {
        kind: EpochSubjectKind::Empty,
        id: Uuid::nil(),
        epoch: Epoch::NEVER,
    };
}

/// The five epoch slots an envelope carries.
///
/// A slot array rather than named fields, because the region needs subject
/// **ids** to consult its local projection and the two principal kinds carry
/// different subjects — `{key, workspace, account}` for a key,
/// `{user, membership, workspace, account}` for an actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EpochSlots([EpochSlot; EPOCH_SLOTS]);

impl EpochSlots {
    /// No subjects at all.
    pub const EMPTY: Self = Self([EpochSlot::EMPTY; EPOCH_SLOTS]);

    /// Builds a slot array from the used slots.
    ///
    /// # Errors
    ///
    /// Returns [`IssueError::TooManyEpochSubjects`] past five and
    /// [`IssueError::DuplicateEpochSubject`] for a repeated
    /// `(kind, id)`. An `Empty` kind in the input is refused too: a caller that
    /// wanted a shorter list should pass a shorter list.
    pub fn new(used: &[EpochSlot]) -> Result<Self, IssueError> {
        if used.len() > EPOCH_SLOTS {
            return Err(IssueError::TooManyEpochSubjects);
        }
        let mut slots = [EpochSlot::EMPTY; EPOCH_SLOTS];
        for (index, slot) in used.iter().enumerate() {
            if slot.kind == EpochSubjectKind::Empty {
                return Err(IssueError::DuplicateEpochSubject);
            }
            if used[..index]
                .iter()
                .any(|earlier| earlier.kind == slot.kind && earlier.id == slot.id)
            {
                return Err(IssueError::DuplicateEpochSubject);
            }
            slots[index] = *slot;
        }
        Ok(Self(slots))
    }

    /// The used slots, in order.
    pub fn used(&self) -> impl Iterator<Item = &EpochSlot> {
        self.0
            .iter()
            .take_while(|slot| slot.kind != EpochSubjectKind::Empty)
    }

    /// Every slot, used or not.
    #[must_use]
    pub const fn all(&self) -> &[EpochSlot; EPOCH_SLOTS] {
        &self.0
    }
}

/// What an assertion claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssertionClaims {
    /// When it was issued.
    pub issued_at_ms: u64,
    /// When it stops verifying.
    pub expires_at_ms: u64,
    /// Who may accept it.
    pub audience: Audience,
    /// Which kind of principal.
    pub principal_kind: PrincipalKind,
    /// Which principal.
    pub principal_id: Uuid,
    /// The credential this assertion is bound to.
    pub credential_binding: [u8; 32],
    /// The organization.
    pub organization_id: Uuid,
    /// The workspace.
    pub workspace_id: Uuid,
    /// The workspace's region. Must equal the audience region.
    pub workspace_region: aex_wire::types::Region,
    /// The account state at issue time.
    pub account_state: AssertedAccountState,
    /// The **effective** scopes, already intersected with role and ceiling. The
    /// edge never re-derives them.
    pub scopes: ScopeSet,
    /// The revocation epochs the edge must check.
    pub epochs: EpochSlots,
}

/// The signing key identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(Uuid);

impl KeyId {
    /// Wraps a key id.
    #[must_use]
    pub const fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// The raw id.
    #[must_use]
    pub const fn get(self) -> Uuid {
        self.0
    }
}

/// Something that can sign an assertion.
pub trait AssertionSigner: Send + Sync {
    /// Which key this signer holds.
    fn kid(&self) -> KeyId;
    /// Signs the 259 covered bytes.
    fn sign(&self, message: &[u8]) -> [u8; 64];
}

/// A local Ed25519 signer.
///
/// The private key is unwrapped once per cold start by the deployable and lives
/// only here. Signing is sub-millisecond and involves no network call, which is
/// the whole point: at the projected refresh rate a per-request KMS asymmetric
/// `Sign` would add round-trip latency and a five-figure annual charge for no
/// extra protection over an artifact that is internal, audience-bound,
/// credential-bound and lives thirty seconds.
pub struct LocalSigner {
    kid: KeyId,
    key: SigningKey,
}

impl LocalSigner {
    /// Builds a signer from raw private-key bytes.
    #[must_use]
    pub fn new(kid: KeyId, secret: &Zeroizing<[u8; 32]>) -> Self {
        Self {
            kid,
            key: SigningKey::from_bytes(secret),
        }
    }

    /// The matching public key.
    #[must_use]
    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }
}

impl std::fmt::Debug for LocalSigner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "LocalSigner {{ kid: {:?}, key: <redacted:32 bytes> }}",
            self.kid
        )
    }
}

impl AssertionSigner for LocalSigner {
    fn kid(&self) -> KeyId {
        self.kid
    }

    fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }
}

/// A signed assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assertion([u8; ASSERTION_ENVELOPE_LEN]);

impl Assertion {
    /// The raw envelope.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; ASSERTION_ENVELOPE_LEN] {
        &self.0
    }

    /// The envelope in canonical unpadded base64url, for the internal wire.
    #[must_use]
    pub fn to_base64url(&self) -> String {
        aex_control_domain::codec::base64url(&self.0)
    }

    /// Parses a transmitted envelope.
    ///
    /// # Errors
    ///
    /// Returns [`VerifyError::BadLength`] for a non-canonical spelling or a
    /// wrong length.
    pub fn from_base64url(text: &str) -> Result<Self, VerifyError> {
        let bytes = aex_control_domain::codec::unbase64url(text).ok_or(VerifyError::BadLength)?;
        <[u8; ASSERTION_ENVELOPE_LEN]>::try_from(bytes.as_slice())
            .map(Self)
            .map_err(|_| VerifyError::BadLength)
    }

    /// The envelope as the internal exchange carries it.
    ///
    /// # Errors
    ///
    /// Returns [`AssertionError::Encoding`] never in practice: the envelope is
    /// a fixed 323 bytes and its canonical encoding is well inside the
    /// transport bound. It is a `Result` because the transport type owns its own
    /// invariant and this crate does not restate it.
    pub fn to_issued(&self) -> Result<IssuedAssertion, AssertionError> {
        IssuedAssertion::new(self.to_base64url())
    }

    /// Parses an envelope out of the internal exchange.
    ///
    /// # Errors
    ///
    /// Returns [`VerifyError::BadLength`] for anything that is not exactly
    /// [`ASSERTION_ENVELOPE_LEN`] bytes.
    pub fn from_issued(issued: &IssuedAssertion) -> Result<Self, VerifyError> {
        Self::from_base64url(issued.as_str())
    }
}

/// Why an assertion could not be issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IssueError {
    /// The requested lifetime exceeded thirty seconds.
    #[error("an assertion may live at most {ASSERTION_MAX_LIFETIME_MS} ms")]
    LifetimeTooLong,
    /// The requested lifetime was zero or negative.
    #[error("an assertion must expire after it is issued")]
    LifetimeNonPositive,
    /// More than five epoch subjects.
    #[error("an assertion carries at most {EPOCH_SLOTS} epoch subjects")]
    TooManyEpochSubjects,
    /// The same `(kind, id)` twice, or an `Empty` kind in the used list.
    #[error("an assertion carries each epoch subject at most once")]
    DuplicateEpochSubject,
    /// The workspace region does not equal the audience region.
    #[error("the workspace region must equal the audience region")]
    RegionMismatch,
    /// A presented-credential principal was aimed at the tool executor, or an
    /// agent session was aimed at a regional edge.
    #[error("this principal kind cannot speak to this audience")]
    PrincipalAudienceMismatch,
}

/// Why an assertion did not verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// Not exactly 323 bytes.
    #[error("an assertion is exactly {ASSERTION_ENVELOPE_LEN} bytes")]
    BadLength,
    /// The magic is not `AEXA`.
    #[error("the envelope magic is wrong")]
    BadMagic,
    /// An unknown envelope version.
    #[error("the envelope version is not supported")]
    BadVersion,
    /// An unknown signature algorithm.
    #[error("the signature algorithm is not supported")]
    BadAlgorithm,
    /// The declared body length is not 235.
    #[error("the declared body length is wrong")]
    BadBodyLength,
    /// No verification key matches the `kid`.
    #[error("the signing key is not known")]
    UnknownKid,
    /// The signature did not verify.
    #[error("the signature did not verify")]
    BadSignature,
    /// `now` is before `issued_at`.
    #[error("the assertion is not valid yet")]
    NotYetValid,
    /// `now` is at or after `expires_at`.
    #[error("the assertion expired")]
    Expired,
    /// A validly signed assertion claiming more than thirty seconds.
    #[error("the assertion claims a lifetime longer than {ASSERTION_MAX_LIFETIME_MS} ms")]
    LifetimeTooLong,
    /// The plane, region or service does not match.
    #[error("the assertion is addressed elsewhere")]
    AudienceMismatch,
    /// The binding does not match the presented credential.
    #[error("the assertion is bound to a different credential")]
    CredentialBindingMismatch,
    /// The workspace region does not equal the audience region.
    #[error("the workspace region does not match the audience region")]
    RegionMismatch,
    /// The principal kind and the audience do not pair.
    ///
    /// The tool executor accepts [`PrincipalKind::AgentSession`] and nothing
    /// else, and every other audience refuses it. Without this, a customer
    /// credential admitted to `tool_exec` on a projected key row would be one
    /// `central-authz` call away from an envelope the executor would resolve a
    /// tenant from.
    #[error("this principal kind cannot speak to this audience")]
    PrincipalAudienceMismatch,
    /// An unknown discriminant in a fixed field.
    #[error("the envelope carries an unknown discriminant")]
    UnknownDiscriminant,
    /// A scope bit outside the registry.
    #[error("the envelope carries a scope outside the registry")]
    UnknownScope,
    /// A used slot after an empty one, or a repeated subject.
    #[error("the epoch slots are malformed")]
    MalformedEpochSlots,
    /// The local projection is ahead of the claimed epoch.
    #[error("the credential was revoked")]
    EpochStale {
        /// Which subject.
        kind: EpochSubjectKind,
        /// Which id.
        id: Uuid,
        /// What the assertion claimed.
        claimed: u64,
        /// What the local projection holds.
        projected: u64,
    },
}

impl From<ScopeError> for VerifyError {
    fn from(_: ScopeError) -> Self {
        Self::UnknownScope
    }
}

/// The one-byte region discriminant.
#[must_use]
const fn region_code(region: aex_wire::types::Region) -> u8 {
    aex_control_domain::cursor::region_code(region)
}

/// Whether a principal kind may speak to an audience.
///
/// One rule, stated once, checked at both ends: the tool executor's audience
/// pairs with [`PrincipalKind::AgentSession`] and with nothing else, and that
/// kind pairs with no other audience. Checked at issue so an issuer bug cannot
/// mint the pair, and again at verify so a verifier does not have to trust that
/// the issuer checked.
#[must_use]
pub const fn principal_pairs_with_audience(
    principal_kind: PrincipalKind,
    audience: AssertionAudience,
) -> bool {
    matches!(audience, AssertionAudience::ToolExec)
        == matches!(principal_kind, PrincipalKind::AgentSession)
}

/// The 259 bytes a signature covers.
#[must_use]
pub fn signing_bytes(kid: KeyId, claims: &AssertionClaims) -> [u8; ASSERTION_SIGNED_LEN] {
    let mut out = [0_u8; ASSERTION_SIGNED_LEN];
    out[0..4].copy_from_slice(&ASSERTION_MAGIC);
    out[4] = ASSERTION_VERSION;
    out[5] = ASSERTION_ALG_ED25519;
    out[6..22].copy_from_slice(kid.get().as_bytes());
    out[22..24].copy_from_slice(&BODY_LEN_BE);

    let body = &mut out[24..];
    body[0..8].copy_from_slice(&claims.issued_at_ms.to_be_bytes());
    body[8..16].copy_from_slice(&claims.expires_at_ms.to_be_bytes());
    body[16] = claims.audience.plane as u8;
    body[17] = region_code(claims.audience.region);
    body[18] = audience_code(claims.audience.service);
    body[19] = claims.principal_kind as u8;
    body[20..36].copy_from_slice(claims.principal_id.as_bytes());
    body[36..68].copy_from_slice(&claims.credential_binding);
    body[68..84].copy_from_slice(claims.organization_id.as_bytes());
    body[84..100].copy_from_slice(claims.workspace_id.as_bytes());
    body[100] = region_code(claims.workspace_region);
    body[101] = claims.account_state as u8;
    body[102..110].copy_from_slice(&claims.scopes.bits().to_be_bytes());
    for (index, slot) in claims.epochs.all().iter().enumerate() {
        let at = 110 + index * EPOCH_SLOT_LEN;
        body[at] = slot.kind as u8;
        body[at + 1..at + 17].copy_from_slice(slot.id.as_bytes());
        body[at + 17..at + 25].copy_from_slice(&slot.epoch.get().to_be_bytes());
    }
    out
}

/// Issues a signed assertion.
///
/// # Errors
///
/// Returns [`IssueError`] for a lifetime outside `(0, 30_000]` ms or a workspace
/// region that does not equal the audience region.
pub fn issue(
    signer: &dyn AssertionSigner,
    claims: &AssertionClaims,
) -> Result<Assertion, IssueError> {
    let lifetime = claims
        .expires_at_ms
        .checked_sub(claims.issued_at_ms)
        .ok_or(IssueError::LifetimeNonPositive)?;
    if lifetime == 0 {
        return Err(IssueError::LifetimeNonPositive);
    }
    if lifetime > ASSERTION_MAX_LIFETIME_MS {
        return Err(IssueError::LifetimeTooLong);
    }
    if claims.workspace_region != claims.audience.region {
        return Err(IssueError::RegionMismatch);
    }
    if !principal_pairs_with_audience(claims.principal_kind, claims.audience.service) {
        return Err(IssueError::PrincipalAudienceMismatch);
    }

    let message = signing_bytes(signer.kid(), claims);
    let signature = signer.sign(&message);
    let mut envelope = [0_u8; ASSERTION_ENVELOPE_LEN];
    envelope[..ASSERTION_SIGNED_LEN].copy_from_slice(&message);
    envelope[ASSERTION_SIGNED_LEN..].copy_from_slice(&signature);
    Ok(Assertion(envelope))
}

/// One public key a region will accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerificationKey {
    /// Which key.
    pub kid: KeyId,
    /// The raw Ed25519 public key.
    pub public_key: [u8; 32],
    /// When the region stops accepting it.
    pub not_after_ms: u64,
}

/// The keys a region accepts, sorted by `kid`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerificationKeySet(Vec<VerificationKey>);

impl VerificationKeySet {
    /// Builds a key set.
    ///
    /// # Errors
    ///
    /// Returns `()` past [`MAX_VERIFICATION_KEYS`]. The bound is what keeps the
    /// unknown-`kid` path from becoming a linear scan over an unbounded list.
    pub fn new(mut keys: Vec<VerificationKey>) -> Result<Self, KeySetError> {
        if keys.len() > MAX_VERIFICATION_KEYS {
            return Err(KeySetError::TooManyKeys);
        }
        keys.sort_unstable_by_key(|key| key.kid);
        keys.dedup_by_key(|key| key.kid);
        Ok(Self(keys))
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many keys the set holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The key for `kid`, if the set holds one that has not lapsed.
    #[must_use]
    pub fn find(&self, kid: KeyId, now_ms: u64) -> Option<&VerificationKey> {
        self.0
            .binary_search_by_key(&kid, |key| key.kid)
            .ok()
            .map(|index| &self.0[index])
            .filter(|key| key.not_after_ms > now_ms)
    }
}

/// Why a key set was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeySetError {
    /// More keys than the bound.
    #[error("a verification key set holds at most {MAX_VERIFICATION_KEYS} keys")]
    TooManyKeys,
}

/// The region's local revocation projection.
pub trait EpochProjection {
    /// The epoch the projection holds for `(kind, id)`.
    fn projected(&self, kind: EpochSubjectKind, id: Uuid) -> Epoch;
}

/// What the edge checks an assertion against.
pub struct VerificationInputs<'a> {
    /// The edge's clock.
    pub now_ms: u64,
    /// The audience this edge is.
    pub audience: Audience,
    /// The binding of the credential actually presented.
    pub credential_binding: &'a [u8; 32],
    /// The local revocation projection.
    pub projection: &'a dyn EpochProjection,
}

/// Verifies an assertion.
///
/// # Errors
///
/// Returns [`VerifyError`] naming the first rule that failed. The order is
/// deliberate: length, magic, version, algorithm and `kid` come first because
/// they are needed to check the signature at all; the signature comes next; and
/// every semantic check comes after, so an unauthenticated caller cannot learn
/// state by probing.
pub fn verify(
    raw: &[u8],
    keys: &VerificationKeySet,
    inputs: &VerificationInputs<'_>,
) -> Result<AssertionClaims, VerifyError> {
    if raw.len() != ASSERTION_ENVELOPE_LEN {
        return Err(VerifyError::BadLength);
    }
    if raw[0..4] != ASSERTION_MAGIC {
        return Err(VerifyError::BadMagic);
    }
    if raw[4] != ASSERTION_VERSION {
        return Err(VerifyError::BadVersion);
    }
    if raw[5] != ASSERTION_ALG_ED25519 {
        return Err(VerifyError::BadAlgorithm);
    }
    if u16::from_be_bytes([raw[22], raw[23]]) as usize != ASSERTION_BODY_LEN {
        return Err(VerifyError::BadBodyLength);
    }
    let kid = KeyId::new(Uuid::from_slice(&raw[6..22]).map_err(|_| VerifyError::UnknownKid)?);
    let key = keys
        .find(kid, inputs.now_ms)
        .ok_or(VerifyError::UnknownKid)?;

    let verifying =
        VerifyingKey::from_bytes(&key.public_key).map_err(|_| VerifyError::UnknownKid)?;
    let signature = ed25519_dalek::Signature::from_bytes(
        &<[u8; 64]>::try_from(&raw[ASSERTION_SIGNED_LEN..]).map_err(|_| VerifyError::BadLength)?,
    );
    verifying
        .verify_strict(&raw[..ASSERTION_SIGNED_LEN], &signature)
        .map_err(|_| VerifyError::BadSignature)?;

    let claims = decode_body(&raw[24..ASSERTION_SIGNED_LEN])?;

    let lifetime = claims
        .expires_at_ms
        .checked_sub(claims.issued_at_ms)
        .ok_or(VerifyError::LifetimeTooLong)?;
    if lifetime > ASSERTION_MAX_LIFETIME_MS {
        return Err(VerifyError::LifetimeTooLong);
    }
    if inputs.now_ms < claims.issued_at_ms {
        return Err(VerifyError::NotYetValid);
    }
    if inputs.now_ms >= claims.expires_at_ms {
        return Err(VerifyError::Expired);
    }
    if claims.audience != inputs.audience {
        return Err(VerifyError::AudienceMismatch);
    }
    if claims.workspace_region != claims.audience.region {
        return Err(VerifyError::RegionMismatch);
    }
    if !principal_pairs_with_audience(claims.principal_kind, claims.audience.service) {
        return Err(VerifyError::PrincipalAudienceMismatch);
    }
    if claims.credential_binding != *inputs.credential_binding {
        return Err(VerifyError::CredentialBindingMismatch);
    }
    for slot in claims.epochs.used() {
        let projected = inputs.projection.projected(slot.kind, slot.id);
        if slot.epoch.is_stale_against(projected) {
            return Err(VerifyError::EpochStale {
                kind: slot.kind,
                id: slot.id,
                claimed: slot.epoch.get(),
                projected: projected.get(),
            });
        }
    }
    Ok(claims)
}

/// Decodes the 235-byte body.
fn decode_body(body: &[u8]) -> Result<AssertionClaims, VerifyError> {
    let read_u64 = |at: usize| -> u64 {
        let mut buffer = [0_u8; 8];
        buffer.copy_from_slice(&body[at..at + 8]);
        u64::from_be_bytes(buffer)
    };
    let read_uuid = |at: usize| -> Result<Uuid, VerifyError> {
        Uuid::from_slice(&body[at..at + 16]).map_err(|_| VerifyError::BadLength)
    };

    let plane = Plane::from_wire(body[16]).ok_or(VerifyError::UnknownDiscriminant)?;
    let region = aex_control_domain::cursor::region_from_code(body[17])
        .ok_or(VerifyError::UnknownDiscriminant)?;
    let service = audience_from_code(body[18]).ok_or(VerifyError::UnknownDiscriminant)?;
    let principal_kind =
        PrincipalKind::from_wire(body[19]).ok_or(VerifyError::UnknownDiscriminant)?;
    let workspace_region = aex_control_domain::cursor::region_from_code(body[100])
        .ok_or(VerifyError::UnknownDiscriminant)?;
    let account_state =
        AssertedAccountState::from_wire(body[101]).ok_or(VerifyError::UnknownDiscriminant)?;
    let scopes = ScopeSet::from_bits(read_u64(102))?;

    let mut binding = [0_u8; 32];
    binding.copy_from_slice(&body[36..68]);

    let mut slots = [EpochSlot::EMPTY; EPOCH_SLOTS];
    let mut seen_empty = false;
    for (index, slot) in slots.iter_mut().enumerate() {
        let at = 110 + index * EPOCH_SLOT_LEN;
        let kind = EpochSubjectKind::from_wire(body[at]).ok_or(VerifyError::UnknownDiscriminant)?;
        let id = read_uuid(at + 1)?;
        let epoch = Epoch::new(read_u64(at + 17));
        if kind == EpochSubjectKind::Empty {
            if id != Uuid::nil() || epoch != Epoch::NEVER {
                return Err(VerifyError::MalformedEpochSlots);
            }
            seen_empty = true;
        } else if seen_empty {
            return Err(VerifyError::MalformedEpochSlots);
        }
        *slot = EpochSlot { kind, id, epoch };
    }
    for (index, slot) in slots.iter().enumerate() {
        if slot.kind == EpochSubjectKind::Empty {
            continue;
        }
        if slots[..index]
            .iter()
            .any(|earlier| earlier.kind == slot.kind && earlier.id == slot.id)
        {
            return Err(VerifyError::MalformedEpochSlots);
        }
    }

    Ok(AssertionClaims {
        issued_at_ms: read_u64(0),
        expires_at_ms: read_u64(8),
        audience: Audience {
            plane,
            region,
            service,
        },
        principal_kind,
        principal_id: read_uuid(20)?,
        credential_binding: binding,
        organization_id: read_uuid(68)?,
        workspace_id: read_uuid(84)?,
        workspace_region,
        account_state,
        scopes,
        epochs: EpochSlots(slots),
    })
}

/// The binding for a workspace-key principal.
#[must_use]
pub fn workspace_key_binding(key_id: Uuid, digest: &PresentedDigest) -> [u8; 32] {
    crate::credential::credential_binding(PrincipalKind::WorkspaceKey as u8, key_id, digest)
}

/// The binding for an account-token principal.
#[must_use]
pub fn account_token_binding(token_id: Uuid, digest: &PresentedDigest) -> [u8; 32] {
    crate::credential::credential_binding(PrincipalKind::AccountActor as u8, token_id, digest)
}

/// The binding for a browser-session principal.
#[must_use]
pub fn user_session_binding(session_id: Uuid, digest: &PresentedDigest) -> [u8; 32] {
    crate::credential::credential_binding(PrincipalKind::UserSession as u8, session_id, digest)
}

/// The domain-separating prefix for an agent-session binding.
const AGENT_SESSION_BINDING_DOMAIN: &[u8] = b"aex/authz/binding/agent-session/v1";

/// The binding for an agent-session principal.
///
/// The other three bindings hash the digest of a credential the customer
/// presented. There is no such credential here, so this one binds the envelope
/// to the **one effect attempt it was minted for** instead. A verifier
/// recomputes it from the request it is holding, which is what makes a captured
/// envelope useless for any other call: within its thirty seconds it
/// re-authorises exactly the attempt it already authorised, and the executor's
/// own conditional update is what decides whether that attempt spends twice.
///
/// This is deliberately **not** a digest over the arguments. Binding the whole
/// body would make the envelope's meaning depend on a canonicalisation both ends
/// have to agree about byte for byte; binding the effect identity does not, and
/// the effect identity is already the thing the journal, the receipt and the
/// idempotency record all key on.
///
/// It is also deliberately **not** over the session. A verifier has to compute
/// the expected binding *before* it has verified anything, so every input has to
/// be one the unverified request already carries — and the request carries no
/// tenant-shaped field, by construction. The effect id is
/// `blake3(agent ‖ seq ‖ kind)[..16]`, so it already names one agent's one step;
/// adding the session would add nothing a verifier could check anyway.
#[must_use]
pub fn agent_session_binding(effect_id: Uuid, attempt: u16) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(AGENT_SESSION_BINDING_DOMAIN);
    hasher.update([PrincipalKind::AgentSession as u8]);
    hasher.update(effect_id.as_bytes());
    hasher.update(attempt.to_be_bytes());
    hasher.finalize().into()
}
