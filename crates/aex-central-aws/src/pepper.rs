//! The credential pepper, split between a lifecycle table and one secret.
//!
//! # The contract (OD-39)
//!
//! One Secrets Manager secret holds pepper **material only**:
//!
//! ```json
//! { "version": 3, "purpose": "identity", "secret": "<base64 32 bytes>" }
//! ```
//!
//! The database owns **lifecycle**. `identity.credential_pepper` (and the
//! matching `control.credential_pepper`) records the version, its purpose, its
//! state and — in `secret_ref` — the Secrets Manager **version id** that holds
//! that version's material.
//!
//! The split is what makes rotation survivable. A stored credential names the
//! pepper version it was computed under, so verifying it means resolving *that
//! version*, not "whatever the secret holds now". Fetching by version id gives
//! exactly that, and lets two peppers coexist for as long as the rotation takes:
//! the retiring one still verifies old credentials while the active one mints
//! new ones. A keystore that read only `AWSCURRENT` would make every credential
//! minted before the rotation unverifiable the instant the secret moved, and
//! would report them as *invalid* — a lie the caller acts on by signing the
//! person out.
//!
//! # Redaction
//!
//! No type here renders material. [`aex_identity_domain::Pepper`] redacts its
//! own `Debug`, the payload struct is not `Debug` at all, every error is built
//! from a fixed string or a service error **code**, and the cache's `Debug`
//! shows counts rather than keys. `tests/security.rs` drives each of those.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use aex_identity_app::ports::{PepperKeystore, PepperPurpose, StoreError};
use aex_identity_domain::{Pepper, PepperVersion};
use async_trait::async_trait;
use aws_sdk_secretsmanager::Client;
use aws_sdk_secretsmanager::error::{ProvideErrorMetadata, SdkError};
use base64::Engine as _;
use zeroize::{Zeroize as _, Zeroizing};

/// How many pepper versions one process keeps resolved at a time.
///
/// A rotation has at most two live versions per purpose and there are two
/// purposes, so four is the working set and eight is one doubling of headroom.
/// The bound exists because an unbounded cache keyed by caller-supplied version
/// is a way to make a process hold every pepper that ever existed: each entry is
/// live key material, so the ceiling is a security property rather than a
/// memory one.
const CACHE_BOUND: usize = 8;

/// The bound [`SecretsManagerPepperKeystore`] holds its resolved versions under.
#[must_use]
pub const fn pepper_cache_bound() -> usize {
    CACHE_BOUND
}

/// The lifecycle state a pepper row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PepperState {
    /// New credentials are minted under it.
    Active,
    /// It still verifies, but mints nothing.
    Retiring,
    /// Nothing references it any more.
    Retired,
}

impl PepperState {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Retiring => "retiring",
            Self::Retired => "retired",
        }
    }

    /// Parses the database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "retiring" => Some(Self::Retiring),
            "retired" => Some(Self::Retired),
            _ => None,
        }
    }

    /// Whether a verifier may still be resolved under it.
    ///
    /// A retired pepper is one the rotation proved nothing references. Serving
    /// it would resurrect material the operator believes is out of use.
    #[must_use]
    pub const fn verifies(self) -> bool {
        matches!(self, Self::Active | Self::Retiring)
    }
}

/// One row of the pepper lifecycle table.
///
/// Deliberately carries no material: this is the half the database owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PepperRecord {
    /// Which version.
    pub version: PepperVersion,
    /// What it peppers.
    pub purpose: PepperPurpose,
    /// Where it is in its lifecycle.
    pub state: PepperState,
    /// The Secrets Manager **version id** holding this version's material.
    pub secret_ref: String,
}

/// Where the pepper lifecycle rows live.
///
/// A trait rather than a concrete Aurora read because the table lives in a
/// different schema for each of the two peppers — `identity.credential_pepper`
/// for `central-identity-api`, `control.credential_pepper` for
/// `central-control-api` — and because a keystore that opened its own database
/// connection would be a second authority on which version is active.
#[async_trait]
pub trait PepperDirectory: Send + Sync + fmt::Debug {
    /// The one row whose state is `active`, for `purpose`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] when no pepper is active, which is a
    /// readiness failure rather than a reason to mint under nothing.
    async fn active(&self, purpose: PepperPurpose) -> Result<PepperRecord, StoreError>;

    /// The row for one version.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] when the version is unknown.
    async fn by_version(
        &self,
        purpose: PepperPurpose,
        version: PepperVersion,
    ) -> Result<PepperRecord, StoreError>;

    /// The active row and every verification-only retiring row for `purpose`.
    ///
    /// Implementations must return a fourth row when one exists so the
    /// keystore can refuse an over-wide startup set rather than silently hide
    /// key material that an operator still considers live.
    async fn verification_set(
        &self,
        purpose: PepperPurpose,
    ) -> Result<Vec<PepperRecord>, StoreError>;
}

/// Material loaded before a signer can serve.
///
/// `current` is the only key used for writes. `retiring` exists solely so
/// bounded, still-live cursors survive a rolling rotation.
pub struct PepperVerificationSet {
    /// The active version and material used for new signatures.
    pub current: (PepperVersion, Pepper),
    /// At most two verification-only versions.
    pub retiring: Vec<(PepperVersion, Pepper)>,
}

/// What one secret version's payload says.
///
/// Not `Debug`, not `Clone` and not `PartialEq`: the only thing that may be done
/// with it is turn it into a [`Pepper`], and `secret` is zeroized on the way.
#[derive(serde::Deserialize)]
struct PepperPayload {
    /// The version this material belongs to.
    version: u32,
    /// What it peppers.
    purpose: String,
    /// The 32 bytes, base64 with standard alphabet and padding.
    secret: String,
}

/// A bounded, insertion-ordered map from secret version id to resolved pepper.
///
/// `Pepper` zeroizes on drop, so eviction and shutdown both clear material
/// without this type doing anything beyond dropping the value.
struct PepperCache {
    entries: Vec<(String, Pepper)>,
    order: VecDeque<String>,
}

impl fmt::Debug for PepperCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Counts only. Naming a key would name a live Secrets Manager version
        // id, and holding the value would render material.
        formatter
            .debug_struct("PepperCache")
            .field("resolved", &self.entries.len())
            .field("bound", &CACHE_BOUND)
            .finish_non_exhaustive()
    }
}

impl PepperCache {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &str) -> Option<Pepper> {
        self.entries
            .iter()
            .find(|(id, _)| id == key)
            .map(|(_, pepper)| pepper.clone())
    }

    fn insert(&mut self, key: String, pepper: Pepper) {
        if self.entries.iter().any(|(id, _)| *id == key) {
            return;
        }
        while self.entries.len() >= CACHE_BOUND {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            // Dropping the entry drops its `Pepper`, which zeroizes.
            self.entries.retain(|(id, _)| *id != oldest);
        }
        self.order.push_back(key.clone());
        self.entries.push((key, pepper));
    }
}

/// The pepper keystore over Secrets Manager and a lifecycle directory.
pub struct SecretsManagerPepperKeystore {
    client: Client,
    secret_id: String,
    directory: Arc<dyn PepperDirectory>,
    cache: Mutex<PepperCache>,
}

impl fmt::Debug for SecretsManagerPepperKeystore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The secret **id** is configuration and appears in the deployable's own
        // environment; the material never does. The cache renders counts.
        formatter
            .debug_struct("SecretsManagerPepperKeystore")
            .field("secret_id", &self.secret_id)
            .field("directory", &self.directory)
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl SecretsManagerPepperKeystore {
    /// Builds the keystore for one secret and one lifecycle table.
    #[must_use]
    pub fn new(
        client: Client,
        secret_id: impl Into<String>,
        directory: Arc<dyn PepperDirectory>,
    ) -> Self {
        Self {
            client,
            secret_id: secret_id.into(),
            directory,
            cache: Mutex::new(PepperCache::new()),
        }
    }

    /// How many versions are resolved right now, for a readiness probe.
    #[must_use]
    pub fn resolved(&self) -> usize {
        self.cache.lock().map_or(0, |cache| cache.entries.len())
    }

    /// Resolves the active pepper for `purpose` and reports whether it loaded.
    ///
    /// This is the start-up probe: a deployable that cannot load its own active
    /// pepper cannot mint a credential, and answering `ready` would admit
    /// traffic it can only fail.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] naming why, without any material.
    pub async fn probe(&self, purpose: PepperPurpose) -> Result<PepperVersion, StoreError> {
        let record = self.directory.active(purpose).await?;
        self.material(&record).await?;
        Ok(record.version)
    }

    /// Loads the one current key and at most two verification-only keys.
    ///
    /// This is the cursor start-up boundary. It refuses missing or duplicate
    /// current versions, duplicate IDs, retired rows, purpose mismatches, and
    /// more than two retiring keys before the process begins serving.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed [`StoreError`] for an invalid lifecycle set or for
    /// material that cannot be fetched and validated.
    pub async fn verification_set(
        &self,
        purpose: PepperPurpose,
    ) -> Result<PepperVerificationSet, StoreError> {
        let records = self.directory.verification_set(purpose).await?;
        let (current, retiring) = validate_verification_set(purpose, records)?;
        let current_material = self.material(&current).await?;
        let mut retiring_material = Vec::with_capacity(retiring.len());
        for record in retiring {
            retiring_material.push((record.version, self.material(&record).await?));
        }
        Ok(PepperVerificationSet {
            current: (current.version, current_material),
            retiring: retiring_material,
        })
    }

    /// The material for one lifecycle row.
    async fn material(&self, record: &PepperRecord) -> Result<Pepper, StoreError> {
        if let Ok(cache) = self.cache.lock()
            && let Some(hit) = cache.get(&record.secret_ref)
        {
            return Ok(hit);
        }
        let pepper = self.fetch(record).await?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(record.secret_ref.clone(), pepper.clone());
        }
        Ok(pepper)
    }

    /// Fetches and validates one secret version.
    async fn fetch(&self, record: &PepperRecord) -> Result<Pepper, StoreError> {
        let output = self
            .client
            .get_secret_value()
            .secret_id(&self.secret_id)
            .version_id(&record.secret_ref)
            .send()
            .await
            .map_err(|error| secrets_error(&error))?;
        // Taking the field moves the one `String` the SDK allocated for the
        // payload, so wrapping it zeroizes that allocation rather than a copy.
        let raw = Zeroizing::new(output.secret_string.ok_or_else(|| {
            StoreError::Decode("the pepper secret version holds no string payload".to_owned())
        })?);
        let mut payload: PepperPayload = serde_json::from_str(&raw).map_err(|_| {
            StoreError::Decode("the pepper payload is not the declared shape".to_owned())
        })?;
        let checked = validate(&payload, record);
        payload.secret.zeroize();
        checked
    }
}

fn validate_verification_set(
    purpose: PepperPurpose,
    records: Vec<PepperRecord>,
) -> Result<(PepperRecord, Vec<PepperRecord>), StoreError> {
    if records.len() > 3 {
        return Err(StoreError::Fatal(
            "a pepper verification set exceeds one active and two retiring versions".to_owned(),
        ));
    }
    let mut current = None;
    let mut retiring = Vec::new();
    let mut versions = Vec::new();
    for record in records {
        if record.purpose != purpose {
            return Err(StoreError::Decode(
                "a pepper verification row names another purpose".to_owned(),
            ));
        }
        if versions.contains(&record.version) {
            return Err(StoreError::Fatal(
                "a pepper verification set contains a duplicate version".to_owned(),
            ));
        }
        versions.push(record.version);
        match record.state {
            PepperState::Active if current.is_none() => current = Some(record),
            PepperState::Active => {
                return Err(StoreError::Fatal(
                    "a pepper verification set contains more than one active version".to_owned(),
                ));
            }
            PepperState::Retiring => retiring.push(record),
            PepperState::Retired => {
                return Err(StoreError::Fatal(
                    "a pepper verification set contains a retired version".to_owned(),
                ));
            }
        }
    }
    let current = current.ok_or(StoreError::NotFound)?;
    if retiring.len() > 2 {
        return Err(StoreError::Fatal(
            "a pepper verification set exceeds two retiring versions".to_owned(),
        ));
    }
    Ok((current, retiring))
}

/// Validates one payload against the row that named it, then decodes it.
///
/// Every refusal names the mismatch and nothing else. `version` and `purpose`
/// are checked because the version id is the only thing binding the two halves:
/// a secret version rewritten with another version's material would otherwise
/// verify nothing and be indistinguishable from a wrong password.
fn validate(payload: &PepperPayload, record: &PepperRecord) -> Result<Pepper, StoreError> {
    if payload.version != u32::from(record.version.get()) {
        return Err(StoreError::Decode(
            "the pepper payload names a different version than the row that referenced it"
                .to_owned(),
        ));
    }
    if payload.purpose != record.purpose.as_str() {
        return Err(StoreError::Decode(
            "the pepper payload names a different purpose than the row that referenced it"
                .to_owned(),
        ));
    }
    let mut decoded = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(payload.secret.as_bytes())
            .map_err(|_| {
                StoreError::Decode("the pepper payload is not canonical base64".to_owned())
            })?,
    );
    let bytes: [u8; 32] = decoded.as_slice().try_into().map_err(|_| {
        StoreError::Decode(format!(
            "a pepper is 32 bytes; this payload decoded to {}",
            decoded.len()
        ))
    })?;
    decoded.zeroize();
    Ok(Pepper::new(bytes))
}

/// Maps a Secrets Manager failure onto the store vocabulary.
///
/// The service **code** crosses the boundary; the service **message** never
/// does. A message is vendor prose that has quoted request content before, and
/// this request's content is a secret identifier.
fn secrets_error<E, R>(error: &SdkError<E, R>) -> StoreError
where
    E: ProvideErrorMetadata,
{
    match error {
        SdkError::ServiceError(service) => match service.err().code().unwrap_or("Unknown") {
            "ResourceNotFoundException" => StoreError::NotFound,
            "AccessDeniedException" | "DecryptionFailure" | "UnrecognizedClientException" => {
                StoreError::PermissionDenied
            }
            "InternalServiceError" | "ThrottlingException" | "TooManyRequestsException" => {
                StoreError::Unavailable
            }
            code => StoreError::Fatal(format!("the pepper secret was refused: {code}")),
        },
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            StoreError::Unavailable
        }
        SdkError::ConstructionFailure(_) => {
            StoreError::Fatal("the pepper secret request could not be constructed".to_owned())
        }
        _ => StoreError::Unavailable,
    }
}

#[async_trait]
impl PepperKeystore for SecretsManagerPepperKeystore {
    async fn active(&self, purpose: PepperPurpose) -> Result<(PepperVersion, Pepper), StoreError> {
        let record = self.directory.active(purpose).await?;
        if record.state != PepperState::Active {
            return Err(StoreError::Fatal(
                "the directory answered a non-active row for the active pepper".to_owned(),
            ));
        }
        let pepper = self.material(&record).await?;
        Ok((record.version, pepper))
    }

    async fn by_version(
        &self,
        purpose: PepperPurpose,
        version: PepperVersion,
    ) -> Result<Pepper, StoreError> {
        let record = self.directory.by_version(purpose, version).await?;
        if !record.state.verifies() {
            // Retired means the rotation counted zero live references. A
            // credential presenting it is one the operator believes cannot
            // exist, so this is a hard failure rather than "invalid".
            return Err(StoreError::NotFound);
        }
        self.material(&record).await
    }
}

#[cfg(test)]
mod tests {
    use super::{CACHE_BOUND, PepperCache, PepperPayload, PepperRecord, PepperState, validate};
    use aex_identity_app::ports::PepperPurpose;
    use aex_identity_domain::{Pepper, PepperVersion};

    fn record(version: u16) -> PepperRecord {
        PepperRecord {
            version: PepperVersion::new(version),
            purpose: PepperPurpose::Identity,
            state: PepperState::Active,
            secret_ref: format!("version-{version}"),
        }
    }

    fn payload(version: u32, purpose: &str, secret: &str) -> PepperPayload {
        PepperPayload {
            version,
            purpose: purpose.to_owned(),
            secret: secret.to_owned(),
        }
    }

    const THIRTY_TWO: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    #[test]
    fn a_payload_matching_its_row_decodes_to_thirty_two_bytes() {
        assert!(validate(&payload(1, "identity", THIRTY_TWO), &record(1)).is_ok());
    }

    #[test]
    fn a_payload_naming_another_version_is_refused() {
        let error = validate(&payload(2, "identity", THIRTY_TWO), &record(1))
            .expect_err("a version mismatch");
        assert!(format!("{error}").contains("version"), "{error}");
    }

    #[test]
    fn a_payload_naming_another_purpose_is_refused() {
        let error = validate(&payload(1, "cursor", THIRTY_TWO), &record(1))
            .expect_err("a purpose mismatch");
        assert!(format!("{error}").contains("purpose"), "{error}");
    }

    #[test]
    fn a_payload_of_the_wrong_length_is_refused_rather_than_padded() {
        for short in [
            "",
            "AAEC",
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8fHw==",
        ] {
            assert!(
                validate(&payload(1, "identity", short), &record(1)).is_err(),
                "{short}"
            );
        }
    }

    #[test]
    fn a_payload_that_is_not_base64_is_refused() {
        let error = validate(&payload(1, "identity", "not base64 at all!!"), &record(1))
            .expect_err("bad encoding");
        assert!(format!("{error}").contains("base64"), "{error}");
    }

    #[test]
    fn only_a_live_state_verifies() {
        assert!(PepperState::Active.verifies());
        assert!(PepperState::Retiring.verifies());
        assert!(
            !PepperState::Retired.verifies(),
            "a retired pepper was proved unreferenced; serving it resurrects it"
        );
    }

    #[test]
    fn every_database_spelling_round_trips() {
        for state in [
            PepperState::Active,
            PepperState::Retiring,
            PepperState::Retired,
        ] {
            assert_eq!(PepperState::parse(state.as_str()), Some(state));
        }
        assert_eq!(PepperState::parse("compromised"), None);
    }

    #[test]
    fn the_cache_never_grows_past_its_bound() {
        let mut cache = PepperCache::new();
        for index in 0..(CACHE_BOUND * 3) {
            cache.insert(
                format!("v{index}"),
                Pepper::new([u8::try_from(index % 251).unwrap_or(0); 32]),
            );
            assert!(
                cache.entries.len() <= CACHE_BOUND,
                "the cache held {} entries",
                cache.entries.len()
            );
        }
        assert_eq!(cache.entries.len(), CACHE_BOUND);
    }

    #[test]
    fn the_cache_returns_what_it_stored_and_evicts_the_oldest_first() {
        let mut cache = PepperCache::new();
        for index in 0..CACHE_BOUND {
            cache.insert(format!("v{index}"), Pepper::new([1_u8; 32]));
        }
        assert!(cache.get("v0").is_some());
        cache.insert("overflow".to_owned(), Pepper::new([2_u8; 32]));
        assert!(cache.get("v0").is_none(), "the oldest entry survived");
        assert!(cache.get("overflow").is_some());
    }

    #[test]
    fn reinserting_a_resolved_version_does_not_duplicate_it() {
        let mut cache = PepperCache::new();
        cache.insert("v1".to_owned(), Pepper::new([1_u8; 32]));
        cache.insert("v1".to_owned(), Pepper::new([2_u8; 32]));
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn the_cache_debug_rendering_names_no_version_and_no_material() {
        let mut cache = PepperCache::new();
        cache.insert("aws-version-id-0001".to_owned(), Pepper::new([9_u8; 32]));
        let rendered = format!("{cache:?}");
        assert!(!rendered.contains("aws-version-id-0001"), "{rendered}");
        assert!(rendered.contains("resolved"), "{rendered}");
    }
}
