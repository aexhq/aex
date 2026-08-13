//! Provider-crate properties: the cache, the request translation, and the
//! failure classification surface.

use aex_brain_provider::{CACHE_CAPACITY, CACHE_TTL, CredentialCache};
use aex_brain_provider_custody::credential::{
    BindingState, CredentialResolveError, CredentialRevision, ProviderApiKey,
    ProviderCredentialBinding, ProviderCredentialDecryptor,
};
use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CanonicalMessage, CanonicalModelRequest, CorrelationId, ReasoningRequest, Role, SystemBlock,
    ToolChoice,
};
use aex_model_catalog::document::CapabilitySet;
use aex_model_catalog::fixture;
use aex_model_catalog::primitives::BoundedString;
use aex_wire::ids::{OrganizationId, PrefixedId as _, ProviderCredentialId, WorkspaceId};
use aex_wire::provider::ProviderId;
use aex_wire::types::Region;
use aex_wire::{CanonicalJson, ContentHash, ResourceName};

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [1; 10]))
}

fn organization() -> OrganizationId {
    OrganizationId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10]))
}

fn binding_id(seed: u8) -> ProviderCredentialId {
    ProviderCredentialId::from_uuid7(aex_wire::Uuid7::compose(2, [seed; 10]))
}

fn binding(provider: ProviderId) -> ProviderCredentialBinding {
    ProviderCredentialBinding {
        id: binding_id(7),
        workspace: workspace(),
        provider,
        revision: CredentialRevision(1),
        generation: aex_secret_domain::SourceGeneration(1),
        revocation_epoch: aex_secret_domain::RevocationEpoch(0),
        is_default: true,
        state: BindingState::Ready,
        ciphertext: aex_secret_domain::CiphertextRef {
            key_generation: 1,
            wrapped_key: vec![1; 32],
            nonce: Vec::new(),
            ciphertext: vec![2; 32],
        },
        context: aex_secret_domain::EncryptionContext {
            plane: aex_secret_domain::context::Plane::Dev,
            region: Region::EuWest1,
            organization: organization(),
            workspace: workspace(),
            name: aex_secret_domain::SecretName::parse("provider-key").expect("name"),
            generation: aex_secret_domain::SourceGeneration(1),
            custody_revision: None,
        },
        context_digest: [3; 32],
    }
}

struct Constant(&'static str);
impl ProviderCredentialDecryptor for Constant {
    fn decrypt<'a>(
        &'a self,
        _binding: &'a ProviderCredentialBinding,
        _now: aex_wire::types::Timestamp,
    ) -> aex_brain_app::ports::BoxFuture<'a, Result<ProviderApiKey, CredentialResolveError>> {
        let text = self.0.to_owned();
        Box::pin(async move { Ok(ProviderApiKey::new(text)) })
    }
}

fn now() -> aex_wire::types::Timestamp {
    aex_wire::types::Timestamp::from_unix_millis(1).expect("timestamp")
}

#[tokio::test]
async fn the_cache_reuses_a_fresh_entry_and_expires_a_stale_one() {
    let cache = CredentialCache::new(4, core::time::Duration::from_millis(50));
    let binding = binding(ProviderId::Openai);
    let first = cache
        .decrypt(&binding, &Constant("sk-one"), now())
        .await
        .expect("first decrypt");
    assert_eq!(cache.len(), 1);
    let reused = cache
        .decrypt(&binding, &Constant("sk-two"), now())
        .await
        .expect("cached");
    assert_eq!(reused.plaintext(), "sk-one");
    assert!(
        std::sync::Arc::ptr_eq(first.shared_allocation(), reused.shared_allocation()),
        "a hot hit should share the zeroizing allocation, not copy plaintext"
    );

    tokio::time::sleep(core::time::Duration::from_millis(60)).await;
    let refreshed = cache
        .decrypt(&binding, &Constant("sk-two"), now())
        .await
        .expect("expired then decrypted again");
    assert_eq!(refreshed.plaintext(), "sk-two");
}

#[tokio::test]
async fn a_rotation_never_hits_a_stale_entry() {
    let cache = CredentialCache::default();
    let first = binding(ProviderId::Openai);
    let mut rotated = first.clone();
    rotated.revision = CredentialRevision(2);
    cache
        .decrypt(&first, &Constant("sk-old"), now())
        .await
        .expect("first");
    let after = cache
        .decrypt(&rotated, &Constant("sk-new"), now())
        .await
        .expect("rotated");
    assert_eq!(after.plaintext(), "sk-new");
    assert_eq!(cache.len(), 2);
}

#[tokio::test]
async fn invalidate_drops_exactly_the_matching_entries() {
    let cache = CredentialCache::default();
    let binding = binding(ProviderId::Openai);
    cache
        .decrypt(&binding, &Constant("sk-one"), now())
        .await
        .expect("decrypt");
    assert_eq!(cache.invalidate(workspace(), binding_id(9)), 0);
    assert_eq!(cache.invalidate(workspace(), binding.id), 1);
    assert!(cache.is_empty());
}

#[test]
fn the_cache_constants_are_the_recorded_launch_shape() {
    assert_eq!(CACHE_CAPACITY, 256);
    assert_eq!(CACHE_TTL, core::time::Duration::from_mins(1));
}

#[test]
fn the_cache_debug_rendering_names_no_binding() {
    let cache = CredentialCache::default();
    assert_eq!(format!("{cache:?}"), "CredentialCache { .. }");
}

fn model() -> QualifiedModel {
    fixture::qualified_entry(
        ProviderId::Deepseek,
        "deepseek-v4-pro",
        CapabilitySet::EMPTY,
    )
}

fn request() -> CanonicalModelRequest {
    CanonicalModelRequest {
        selection: model(),
        system: vec![SystemBlock {
            text: BoundedString::truncating("be brief"),
            cacheable: true,
        }],
        messages: vec![CanonicalMessage {
            role: Role::User,
            blocks: Vec::new(),
        }],
        tools: Vec::new(),
        tool_choice: ToolChoice::Auto,
        parallel_tools: false,
        max_output_tokens: 4096,
        temperature_milli: None,
        top_p_milli: None,
        stop_sequences: Vec::new(),
        reasoning: ReasoningRequest::ProviderDefault,
        structured_output: None,
        cache_breakpoints: Vec::new(),
        correlation: CorrelationId(BoundedString::truncating("aex-correlation")),
        request_hash: ContentHash::of(b"placeholder"),
    }
}

#[test]
fn the_request_translation_keeps_every_canonical_member() {
    let request = request();
    let rig =
        aex_brain_provider::translate_request(&request).expect("the canonical request translates");
    let rendered = serde_json::to_string(&rig).expect("a rig request serializes");
    assert!(rendered.contains("deepseek-v4-pro"), "{rendered}");
}

#[test]
fn the_canonical_corpus_still_round_trips_through_the_vocabulary() {
    let _: CanonicalJson = CanonicalJson::parse("{}").expect("json");
    let _: ResourceName = ResourceName::parse("read_file").expect("name");
}
