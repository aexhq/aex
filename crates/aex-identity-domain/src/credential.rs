//! The credential codec and verifier.
//!
//! Five AEX-minted credentials share one primitive. There is no invitation
//! credential: invitations carry no secret at all.
//!
//! ```text
//! workspace key      aex_wk_<region>_<26 workspace>_<26 key>_<43 base64url>
//! account token      aex_at_<26>_<43>
//! dashboard session  aex_ds_<26>_<43>
//! email challenge    aex_ec_<26>_<43>
//! device code        aex_dc_<26>_<43>
//! ```
//!
//! # Why it is shaped this way
//!
//! The secret is exactly 32 CSPRNG bytes rendered as canonical unpadded
//! base64url, so the final character can only encode a four-bit remainder and an
//! alternate textual spelling of the same bytes is refused. The 26-character
//! id suffix is Crockford base32 whose leading character is at most `'7'`, so
//! 130 encoded bits cannot overflow a 128-bit payload.
//!
//! A workspace key carries its workspace as well as its region because it is the
//! one credential presented to a regional host. Both facts have to be readable
//! from the token itself, or the region cannot name the rows it must read until
//! after a central round trip has told it which workspace the key belongs to.
//! Neither segment is a capability: both are public identifiers, and the secret
//! is still the only thing that authenticates.
//!
//! The stored value is `HMAC-SHA256(pepper_v, SHA-256(complete token))`. Two
//! deliberate strengthenings over the system this replaces: it is a **keyed**
//! MAC rather than an unkeyed `SHA-256(pepper ‖ ":" ‖ token)` prefix
//! construction, and because the MAC is over a digest rather than the token, a
//! regional edge can send `{keyId, presentedDigest}` and the customer's
//! plaintext never crosses a region boundary or reaches the central plane.
//!
//! There is no password KDF. The secret is uniformly random 256 bits, so a
//! memory-hard KDF would add nothing against guessing while turning a flood of
//! invalid tokens into CPU exhaustion.
//!
//! [`parse`] performs only structural checks and never compares a secret, so a
//! parse failure leaks nothing about whether a credential exists.

use std::fmt;

use aex_control_domain::codec::{FINAL_QUARTET_ALPHABET, base64url, unbase64url};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use uuid::Uuid;
use zeroize::Zeroizing;

/// Which credential a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CredentialKind {
    /// A workspace API key, region-pinned.
    WorkspaceKey,
    /// An account token minted by the device flow.
    AccountToken,
    /// A browser session.
    DashboardSession,
    /// A single-use email sign-in link.
    EmailChallenge,
    /// A device-flow device code.
    DeviceCode,
}

impl CredentialKind {
    /// Every kind.
    pub const ALL: [Self; 5] = [
        Self::WorkspaceKey,
        Self::AccountToken,
        Self::DashboardSession,
        Self::EmailChallenge,
        Self::DeviceCode,
    ];

    /// The literal prefix, including the trailing underscore.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::WorkspaceKey => "aex_wk_",
            Self::AccountToken => "aex_at_",
            Self::DashboardSession => "aex_ds_",
            Self::EmailChallenge => "aex_ec_",
            Self::DeviceCode => "aex_dc_",
        }
    }

    /// Whether the kind carries a region code and a workspace id.
    ///
    /// Only a workspace key does: it is the one credential a client presents to
    /// a regional host, so both have to be readable without a lookup.
    #[must_use]
    pub const fn carries_workspace_pin(self) -> bool {
        matches!(self, Self::WorkspaceKey)
    }

    /// The exact character length of a token of this kind.
    ///
    /// A workspace key's length depends on its region code, which is four or
    /// five characters, so the pin is a parameter rather than an assumption.
    #[must_use]
    pub fn token_len(self, pin: Option<WorkspacePin>) -> usize {
        let base = self.prefix().len() + ID_LEN + 1 + SECRET_TEXT_LEN;
        match pin.filter(|_| self.carries_workspace_pin()) {
            Some(pin) => base + pin.region.as_str().len() + 1 + ID_LEN + 1,
            None => base,
        }
    }
}

/// Where a workspace key is pinned.
///
/// One value rather than two arguments, because the region and the workspace are
/// never independently true of a credential: a key minted for a workspace is
/// minted for the region that workspace is placed in, and a caller that could
/// supply one without the other could mint a token naming a workspace no region
/// would ever serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspacePin {
    /// The regional endpoint that serves the key.
    pub region: RegionCode,
    /// The workspace the key authorizes.
    pub workspace: Uuid,
}

/// A short region code, as it appears inside a workspace key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionCode(aex_wire::types::Region);

impl RegionCode {
    /// Every code, in wire-contract order.
    pub const ALL: [Self; 5] = [
        Self(aex_wire::types::Region::UsEast1),
        Self(aex_wire::types::Region::UsEast2),
        Self(aex_wire::types::Region::UsWest2),
        Self(aex_wire::types::Region::ApNortheast1),
        Self(aex_wire::types::Region::EuWest1),
    ];

    /// Wraps a region.
    #[must_use]
    pub const fn new(region: aex_wire::types::Region) -> Self {
        Self(region)
    }

    /// The region.
    #[must_use]
    pub const fn region(self) -> aex_wire::types::Region {
        self.0
    }

    /// The short code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self.0 {
            aex_wire::types::Region::UsEast1 => "use1",
            aex_wire::types::Region::UsEast2 => "use2",
            aex_wire::types::Region::UsWest2 => "usw2",
            aex_wire::types::Region::ApNortheast1 => "apne1",
            aex_wire::types::Region::EuWest1 => "euw1",
        }
    }

    /// Resolves a short code.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// `SHA-256` over the complete token's UTF-8 bytes.
///
/// This is what a regional edge transmits. It is not a verifier: without the
/// pepper it proves nothing, and holding it does not let its holder authenticate.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentedDigest([u8; 32]);

impl PresentedDigest {
    /// Computes the digest of a complete token.
    #[must_use]
    pub fn of(token: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        Self(hasher.finalize().into())
    }

    /// Wraps a transmitted digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The digest in canonical unpadded base64url, for the internal wire.
    #[must_use]
    pub fn to_base64url(&self) -> String {
        base64url(&self.0)
    }

    /// Parses a transmitted digest.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::BadSecretEncoding`] for a non-canonical
    /// spelling or a wrong length.
    pub fn from_base64url(text: &str) -> Result<Self, CredentialError> {
        let bytes = unbase64url(text).ok_or(CredentialError::BadSecretEncoding)?;
        <[u8; 32]>::try_from(bytes.as_slice())
            .map(Self)
            .map_err(|_| CredentialError::BadLength)
    }
}

impl fmt::Debug for PresentedDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted:32 bytes>")
    }
}

/// The value a credential row stores: `HMAC-SHA256(pepper, presented_digest)`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Verifier([u8; 32]);

impl Verifier {
    /// Wraps a stored verifier.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw verifier, for the `bytea` column.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Verifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted:32 bytes>")
    }
}

/// Which pepper a verifier was computed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PepperVersion(u16);

impl PepperVersion {
    /// Wraps a stored version.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// The stored value.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// A credential pepper.
#[derive(Clone)]
pub struct Pepper(Zeroizing<[u8; 32]>);

impl Pepper {
    /// Wraps 32 secret bytes.
    #[must_use]
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Copies the material into another secret-bearing domain type.
    ///
    /// The explicit verb keeps composition-root use reviewable; callers must
    /// never render or persist the returned bytes.
    #[must_use]
    pub fn expose_copy(&self) -> [u8; 32] {
        *self.0
    }
}

impl fmt::Debug for Pepper {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted:32 bytes>")
    }
}

/// A freshly minted plaintext credential, returned exactly once.
#[derive(Clone)]
pub struct MintedSecret(Zeroizing<String>);

impl MintedSecret {
    /// The plaintext, for the single response that returns it.
    ///
    /// Named `expose` rather than `as_str` so every call site reads as a
    /// deliberate act in review.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MintedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<redacted:{} bytes>", self.0.len())
    }
}

/// A structurally valid credential, before any secret comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedCredential {
    /// Which kind it is.
    pub kind: CredentialKind,
    /// Its region and workspace, for a workspace key.
    pub pin: Option<WorkspacePin>,
    /// The row id to look up. This is a primary-key hit, so a forged id costs
    /// one index miss rather than a scan.
    pub id: Uuid,
    /// The digest to verify against the stored verifier.
    pub digest: PresentedDigest,
}

/// Why a credential was refused, structurally.
///
/// Every arm is reachable from a mutated valid token, and none of them depends
/// on whether the credential exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    /// The prefix is not the expected kind's.
    #[error("the credential does not have the expected prefix")]
    WrongPrefix,
    /// The region segment is not a known short code.
    #[error("the credential names an unknown region")]
    WrongRegionCode,
    /// The wrong number of `_`-separated segments.
    #[error("the credential is not well formed")]
    BadShape,
    /// The 26-character id suffix did not decode.
    #[error("the credential id segment is not canonical Crockford base32")]
    BadIdEncoding,
    /// The 43-character secret did not decode canonically.
    #[error("the credential secret segment is not canonical base64url")]
    BadSecretEncoding,
    /// The overall length is wrong for the kind.
    #[error("the credential has the wrong length")]
    BadLength,
}

/// A cryptographically secure random source.
///
/// A trait rather than a concrete generator so the domain reads no RNG: the
/// deployables inject one, and the tests inject a recorded one.
pub trait SecretRng: Send + Sync {
    /// Fills `out` with cryptographically secure random bytes.
    fn fill(&self, out: &mut [u8]);
}

/// How many bytes a credential secret holds.
pub const SECRET_BYTES: usize = 32;
/// How many characters 32 bytes render as in unpadded base64url.
pub const SECRET_TEXT_LEN: usize = 43;
/// How many characters a 128-bit id renders as in Crockford base32.
pub const ID_LEN: usize = 26;

/// Crockford base32, lowercase, matching `aex_wire::ids`.
const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Renders a `UUID` payload as 26 Crockford characters.
#[must_use]
pub fn encode_id(id: Uuid) -> String {
    let bits = id.as_u128();
    let mut out = [0_u8; ID_LEN];
    for (index, slot) in out.iter_mut().enumerate() {
        let shift = 5 * (ID_LEN - 1 - index);
        let value = if shift >= 128 {
            0
        } else {
            ((bits >> shift) & 0x1f) as usize
        };
        *slot = CROCKFORD[value];
    }
    String::from_utf8(out.to_vec()).unwrap_or_default()
}

/// Decodes 26 Crockford characters into a `UUID` payload.
///
/// The leading character must be at most `'7'`, because 26 characters carry 130
/// bits and only 128 fit. A lenient decoder would let four different spellings
/// name the same row.
#[must_use]
pub fn decode_id(text: &str) -> Option<Uuid> {
    let bytes = text.as_bytes();
    if bytes.len() != ID_LEN {
        return None;
    }
    let mut bits = 0_u128;
    for (index, byte) in bytes.iter().enumerate() {
        let value = CROCKFORD.iter().position(|it| it == byte)? as u128;
        if index == 0 && value > 7 {
            return None;
        }
        bits = (bits << 5) | value;
    }
    Some(Uuid::from_u128(bits))
}

/// Mints a credential.
///
/// Returns the complete plaintext — the only time it exists — and the digest to
/// compute a verifier from.
#[must_use]
pub fn mint(
    kind: CredentialKind,
    pin: Option<WorkspacePin>,
    id: Uuid,
    rng: &dyn SecretRng,
) -> (MintedSecret, PresentedDigest) {
    let mut secret = Zeroizing::new([0_u8; SECRET_BYTES]);
    rng.fill(secret.as_mut());
    let mut token = String::with_capacity(kind.token_len(pin));
    token.push_str(kind.prefix());
    if let Some(pin) = pin.filter(|_| kind.carries_workspace_pin()) {
        token.push_str(pin.region.as_str());
        token.push('_');
        token.push_str(&encode_id(pin.workspace));
        token.push('_');
    }
    token.push_str(&encode_id(id));
    token.push('_');
    token.push_str(&base64url(secret.as_ref()));
    let digest = PresentedDigest::of(&token);
    (MintedSecret(Zeroizing::new(token)), digest)
}

/// Parses a credential, checking structure only.
///
/// # Errors
///
/// Returns [`CredentialError`] naming the structural rule that failed. No arm
/// depends on database state, so a caller learns nothing about existence.
pub fn parse(expected: CredentialKind, raw: &str) -> Result<ParsedCredential, CredentialError> {
    let body = raw
        .strip_prefix(expected.prefix())
        .ok_or(CredentialError::WrongPrefix)?;

    let (pin, rest) = if expected.carries_workspace_pin() {
        let (code, rest) = body.split_once('_').ok_or(CredentialError::BadShape)?;
        let region = RegionCode::parse(code).ok_or(CredentialError::WrongRegionCode)?;
        let (workspace_text, rest) = split_id(rest)?;
        let workspace = decode_id(workspace_text).ok_or(CredentialError::BadIdEncoding)?;
        (Some(WorkspacePin { region, workspace }), rest)
    } else {
        (None, body)
    };

    let (id_text, secret_text) = split_id(rest)?;
    if secret_text.len() != SECRET_TEXT_LEN {
        return Err(CredentialError::BadLength);
    }
    let id = decode_id(id_text).ok_or(CredentialError::BadIdEncoding)?;
    if !secret_text
        .chars()
        .next_back()
        .is_some_and(|last| FINAL_QUARTET_ALPHABET.contains(last))
    {
        return Err(CredentialError::BadSecretEncoding);
    }
    let secret = unbase64url(secret_text).ok_or(CredentialError::BadSecretEncoding)?;
    if secret.len() != SECRET_BYTES {
        return Err(CredentialError::BadLength);
    }

    Ok(ParsedCredential {
        kind: expected,
        pin,
        id,
        digest: PresentedDigest::of(raw),
    })
}

/// Takes one fixed-width id segment and its separator from the head of `text`.
///
/// Every boundary after the region is taken at a fixed offset rather than by
/// searching, because canonical base64url legitimately contains `_` and a
/// searching split would let a secret's own separator be read as a segment
/// boundary — which, with two id segments, is no longer merely wrong but
/// ambiguous.
fn split_id(text: &str) -> Result<(&str, &str), CredentialError> {
    let head = text.get(..ID_LEN).ok_or(CredentialError::BadLength)?;
    if text.as_bytes().get(ID_LEN) != Some(&b'_') {
        return Err(CredentialError::BadLength);
    }
    let tail = text.get(ID_LEN + 1..).ok_or(CredentialError::BadLength)?;
    Ok((head, tail))
}

/// The domain-separating prefix for a credential verifier.
const VERIFIER_DOMAIN: &[u8] = b"aex/credential/verifier/v1\x1f";

/// Computes the verifier for a presented digest under `pepper`.
///
/// # Panics
///
/// Never: HMAC-SHA256 accepts a key of any length, and a pepper is 32 bytes.
#[must_use]
pub fn verifier(pepper: &Pepper, digest: &PresentedDigest) -> Verifier {
    let mut hmac = <Hmac<Sha256> as KeyInit>::new_from_slice(pepper.0.as_ref())
        .expect("HMAC-SHA256 accepts a 32-byte key");
    hmac.update(VERIFIER_DOMAIN);
    hmac.update(digest.as_bytes());
    Verifier(hmac.finalize().into_bytes().into())
}

/// Verifies a presented digest against a stored verifier, in constant time.
#[must_use]
pub fn verify(pepper: &Pepper, digest: &PresentedDigest, stored: &Verifier) -> bool {
    verifier(pepper, digest).0.ct_eq(&stored.0).unwrap_u8() == 1
}

/// The domain-separating prefix for the credential binding.
const BINDING_DOMAIN: &[u8] = b"aex/authz/binding/v1";

/// The 32-byte binding an assertion carries.
///
/// Binding the assertion to the exact credential that produced it is what stops
/// a leaked assertion from being useful to anyone who does not also hold the
/// credential.
#[must_use]
pub fn credential_binding(
    principal_kind: u8,
    principal_id: Uuid,
    digest: &PresentedDigest,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(BINDING_DOMAIN);
    hasher.update([principal_kind]);
    hasher.update(principal_id.as_bytes());
    hasher.update(digest.as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialError, CredentialKind, ID_LEN, Pepper, PresentedDigest, RegionCode,
        SECRET_TEXT_LEN, SecretRng, WorkspacePin, credential_binding, decode_id, encode_id, mint,
        parse, verifier, verify,
    };
    use uuid::Uuid;

    /// A recorded generator: the domain reads no RNG, so a test supplies one.
    struct Fixed(u8);

    impl SecretRng for Fixed {
        fn fill(&self, out: &mut [u8]) {
            out.fill(self.0);
        }
    }

    fn id() -> Uuid {
        Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0001)
    }

    fn workspace() -> Uuid {
        Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0002)
    }

    fn pin(region: aex_wire::types::Region) -> WorkspacePin {
        WorkspacePin {
            region: RegionCode::new(region),
            workspace: workspace(),
        }
    }

    #[test]
    fn every_kind_has_a_distinct_seven_character_prefix() {
        let mut prefixes: Vec<&str> = CredentialKind::ALL.iter().map(|it| it.prefix()).collect();
        assert!(prefixes.iter().all(|it| it.len() == 7), "{prefixes:?}");
        prefixes.sort_unstable();
        prefixes.dedup();
        assert_eq!(prefixes.len(), CredentialKind::ALL.len());
    }

    #[test]
    fn only_a_workspace_key_carries_a_workspace_pin() {
        for kind in CredentialKind::ALL {
            assert_eq!(
                kind.carries_workspace_pin(),
                kind == CredentialKind::WorkspaceKey,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn the_golden_shapes_are_exact() {
        let (secret, _) = mint(
            CredentialKind::WorkspaceKey,
            Some(pin(aex_wire::types::Region::EuWest1)),
            id(),
            &Fixed(0),
        );
        let token = secret.expose();
        assert!(token.starts_with("aex_wk_euw1_"), "{token}");
        assert_eq!(
            token.len(),
            7 + 4 + 1 + ID_LEN + 1 + ID_LEN + 1 + SECRET_TEXT_LEN
        );
        assert_eq!(
            token.len(),
            CredentialKind::WorkspaceKey.token_len(Some(pin(aex_wire::types::Region::EuWest1)))
        );

        for kind in [
            CredentialKind::AccountToken,
            CredentialKind::DashboardSession,
            CredentialKind::EmailChallenge,
            CredentialKind::DeviceCode,
        ] {
            let (secret, _) = mint(kind, None, id(), &Fixed(0));
            let token = secret.expose();
            assert!(token.starts_with(kind.prefix()), "{token}");
            assert_eq!(token.len(), 77, "{kind:?} renders as 77 characters");
            assert_eq!(token.len(), kind.token_len(None));
        }
    }

    #[test]
    fn mint_parse_verify_round_trips_for_every_kind() {
        let pepper = Pepper::new([9_u8; 32]);
        for kind in CredentialKind::ALL {
            let pinned = kind
                .carries_workspace_pin()
                .then(|| pin(aex_wire::types::Region::UsWest2));
            let (secret, digest) = mint(kind, pinned, id(), &Fixed(7));
            let parsed = parse(kind, secret.expose()).expect("a minted token parses");
            assert_eq!(parsed.kind, kind);
            assert_eq!(parsed.id, id());
            assert_eq!(parsed.pin, pinned);
            assert_eq!(parsed.digest, digest);
            let stored = verifier(&pepper, &digest);
            assert!(verify(&pepper, &parsed.digest, &stored));
        }
    }

    #[test]
    fn a_secret_bearing_a_separator_never_shifts_a_workspace_key_segment() {
        // 0xff renders base64url `_____…`, so every fixed-width boundary in the
        // token is followed and preceded by the secret's own separators.
        let pinned = pin(aex_wire::types::Region::ApNortheast1);
        let (secret, _) = mint(
            CredentialKind::WorkspaceKey,
            Some(pinned),
            id(),
            &Fixed(0xff),
        );
        let token = secret.expose();
        let tail = &token[token.len() - SECRET_TEXT_LEN..];
        assert!(tail.contains('_'), "{tail}");
        let parsed = parse(CredentialKind::WorkspaceKey, token).expect("parses");
        assert_eq!(parsed.pin, Some(pinned));
        assert_eq!(parsed.id, id());
    }

    #[test]
    fn a_token_of_one_kind_never_parses_as_another() {
        let (secret, _) = mint(CredentialKind::AccountToken, None, id(), &Fixed(1));
        for kind in CredentialKind::ALL {
            if kind == CredentialKind::AccountToken {
                continue;
            }
            assert_eq!(
                parse(kind, secret.expose()),
                Err(CredentialError::WrongPrefix),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn every_structural_error_is_reachable_from_a_mutated_valid_token() {
        let (secret, _) = mint(
            CredentialKind::WorkspaceKey,
            Some(pin(aex_wire::types::Region::EuWest1)),
            id(),
            &Fixed(3),
        );
        let token = secret.expose().to_owned();

        assert_eq!(
            parse(
                CredentialKind::WorkspaceKey,
                &token.replace("aex_wk_", "aex_zz_")
            ),
            Err(CredentialError::WrongPrefix)
        );
        assert_eq!(
            parse(CredentialKind::WorkspaceKey, &token.replace("euw1", "zzz9")),
            Err(CredentialError::WrongRegionCode)
        );
        assert_eq!(
            parse(CredentialKind::WorkspaceKey, "aex_wk_euw1"),
            Err(CredentialError::BadShape)
        );
        assert_eq!(
            parse(CredentialKind::WorkspaceKey, &format!("{token}x")),
            Err(CredentialError::BadLength)
        );

        let workspace_start = "aex_wk_euw1_".len();
        let id_start = workspace_start + ID_LEN + 1;
        for start in [workspace_start, id_start] {
            let mut mutated = token.clone();
            mutated.replace_range(start..=start, "9");
            assert_eq!(
                parse(CredentialKind::WorkspaceKey, &mutated),
                Err(CredentialError::BadIdEncoding),
                "a leading Crockford character above `7` overflows 128 bits"
            );

            let mut mutated = token.clone();
            mutated.replace_range(start..=start, "i");
            assert_eq!(
                parse(CredentialKind::WorkspaceKey, &mutated),
                Err(CredentialError::BadIdEncoding),
                "the excluded Crockford letters are not folded onto digits"
            );
        }

        // Deleting one character from the workspace segment slides the key
        // segment left; the fixed-width boundary refuses it rather than reading
        // a different key.
        let mut mutated = token.clone();
        mutated.remove(workspace_start);
        assert_eq!(
            parse(CredentialKind::WorkspaceKey, &mutated),
            Err(CredentialError::BadLength)
        );

        let mut mutated = token;
        let last = mutated.len() - 1;
        mutated.replace_range(last.., "B");
        assert_eq!(
            parse(CredentialKind::WorkspaceKey, &mutated),
            Err(CredentialError::BadSecretEncoding),
            "the final character can only encode a four-bit remainder"
        );
    }

    #[test]
    fn a_separator_moved_inside_the_id_is_refused() {
        let (secret, _) = mint(CredentialKind::AccountToken, None, id(), &Fixed(2));
        let token = secret.expose().to_owned();
        // Move the separator one character earlier: the id segment is then 25
        // characters, which no credential has.
        let mut mutated = token.clone();
        mutated.replace_range((7 + ID_LEN - 1)..=(7 + ID_LEN), "_0");
        assert_eq!(
            parse(CredentialKind::AccountToken, &mutated),
            Err(CredentialError::BadLength)
        );
    }

    #[test]
    fn a_secret_containing_a_separator_still_parses() {
        // Canonical base64url uses `-` and `_`; a secret that happens to render
        // one must not be mistaken for a segment boundary.
        let (secret, digest) = mint(CredentialKind::AccountToken, None, id(), &Fixed(0xff));
        let token = secret.expose();
        let tail = &token[7 + ID_LEN + 1..];
        assert!(tail.contains('_') || tail.contains('-'), "{tail}");
        let parsed = parse(CredentialKind::AccountToken, token).expect("parses");
        assert_eq!(parsed.digest, digest);
    }

    #[test]
    fn the_id_codec_round_trips_and_rejects_an_overflowing_spelling() {
        for bits in [
            0_u128,
            1,
            u128::MAX,
            0x0192_3f2a_1c00_7000_8000_0000_0000_0001,
        ] {
            let id = Uuid::from_u128(bits);
            assert_eq!(decode_id(&encode_id(id)), Some(id));
        }
        let mut overflowing = encode_id(Uuid::from_u128(u128::MAX));
        overflowing.replace_range(0..1, "8");
        assert_eq!(decode_id(&overflowing), None);
        assert_eq!(decode_id("short"), None);
    }

    #[test]
    fn a_wrong_pepper_never_verifies() {
        let (_, digest) = mint(CredentialKind::DeviceCode, None, id(), &Fixed(4));
        let stored = verifier(&Pepper::new([1_u8; 32]), &digest);
        assert!(!verify(&Pepper::new([2_u8; 32]), &digest, &stored));
        assert!(verify(&Pepper::new([1_u8; 32]), &digest, &stored));
    }

    #[test]
    fn a_wrong_digest_never_verifies() {
        let pepper = Pepper::new([1_u8; 32]);
        let (_, digest) = mint(CredentialKind::DeviceCode, None, id(), &Fixed(4));
        let stored = verifier(&pepper, &digest);
        let other = PresentedDigest::of("aex_dc_something_else");
        assert!(!verify(&pepper, &other, &stored));
    }

    #[test]
    fn the_digest_transmits_and_returns_unchanged() {
        let (_, digest) = mint(
            CredentialKind::WorkspaceKey,
            Some(WorkspacePin {
                region: RegionCode::ALL[4],
                workspace: workspace(),
            }),
            id(),
            &Fixed(5),
        );
        let text = digest.to_base64url();
        assert_eq!(text.len(), 43);
        assert_eq!(PresentedDigest::from_base64url(&text), Ok(digest));
        assert_eq!(
            PresentedDigest::from_base64url("not base64url!"),
            Err(CredentialError::BadSecretEncoding)
        );
    }

    #[test]
    fn the_binding_separates_principal_kind_id_and_digest() {
        let (_, digest) = mint(
            CredentialKind::WorkspaceKey,
            Some(WorkspacePin {
                region: RegionCode::ALL[4],
                workspace: workspace(),
            }),
            id(),
            &Fixed(6),
        );
        let base = credential_binding(1, id(), &digest);
        assert_ne!(base, credential_binding(2, id(), &digest));
        assert_ne!(base, credential_binding(1, Uuid::from_u128(2), &digest));
        assert_ne!(
            base,
            credential_binding(1, id(), &PresentedDigest::of("other"))
        );
    }

    #[test]
    fn no_secret_type_renders_its_bytes() {
        let (secret, digest) = mint(CredentialKind::AccountToken, None, id(), &Fixed(8));
        let pepper = Pepper::new([1_u8; 32]);
        let stored = verifier(&pepper, &digest);
        for rendered in [
            format!("{secret:?}"),
            format!("{digest:?}"),
            format!("{pepper:?}"),
            format!("{stored:?}"),
        ] {
            assert!(rendered.starts_with("<redacted:"), "{rendered}");
            assert!(!rendered.contains("aex_at_"), "{rendered}");
        }
    }

    #[test]
    fn the_region_codes_round_trip() {
        for code in RegionCode::ALL {
            assert_eq!(RegionCode::parse(code.as_str()), Some(code));
            assert_eq!(RegionCode::new(code.region()), code);
        }
        assert_eq!(RegionCode::parse("eu-west-1"), None);
        assert_eq!(RegionCode::parse(""), None);
    }
}
