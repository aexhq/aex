//! Protected qualification plan for the first real model-catalog entry.
//!
//! This module owns no provider default. It is deliberately specific to the
//! reviewed genesis target and keeps unsupported live probes visible rather
//! than manufacturing a receipt from adapter fixtures.

use core::fmt;
use core::time::Duration;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aex_brain_application::ports::{BoxFuture, CancelToken, NullPreviewSink};
use aex_brain_provider_gateway::DispatchProof;
use aex_brain_provider_gateway::adapter::{
    BoundedBody, HeaderView, ProviderAdapter, SealedResponse,
};
use aex_brain_provider_gateway::budget::{BudgetOverrun, StreamBudget};
use aex_brain_provider_gateway::credential::ProviderApiKey;
use aex_brain_provider_gateway::deepseek::{
    DeepSeekAdapter, QualificationRequest, build_non_streamed_qualification_request,
    build_qualification_request, decode_non_streamed_qualification_response,
};
use aex_brain_provider_gateway::error::{ProviderFailureKind, RateLimitFeedback, RedactedDetail};
use aex_brain_provider_gateway::stream::{
    ConsumedStream, NullResponseStartSink, QualificationStreamError, ResponseStartSink,
    ResponseStartSinkError, StreamConsumeError, StreamFailure,
    consume_deepseek_qualification_stream,
};
use aex_brain_provider_gateway::transport::{
    QualificationSendFault, SendState, WireRequest, execute, execute_qualification_fault,
};
use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalMessage, NormalizedUsage, ReasoningRequest, Role, ToolChoice,
    UsageCompleteness,
};
use aex_model_catalog::document::{EndpointPin, EntryState, ModelEntry};
use aex_model_catalog::primitives::{BoundedString, ModelSlug, ProviderRequestId};
use aex_model_catalog::receipt::ProbeId;
use aex_wire::provider::ProviderId;
use aex_wire::{ContentHash, to_jcs_bytes};
use base64::Engine as _;
use bytes::Bytes;
use futures::{StreamExt as _, stream};
use sha2::{Digest as _, Sha256};

use crate::ProbeRun;
use crate::evidence::{ErrorBodyShape, ProbeEvidence};
use crate::executor::{
    ConfiguredProbeExecutor, ProbeDriver, ProbeDriverFuture, ProbeProgram, QualificationTarget,
    ReadinessError, prepare_with_credential_presence,
};
pub use crate::genesis::{
    DEEPSEEK_FLASH_MODEL, DEEPSEEK_GENESIS_CAPABILITIES as GENESIS_CAPABILITIES,
};
use crate::tokenizer_oracle::{
    DeepSeekCorpusPair, EIGHTY_PERCENT_CORPUS_DIGEST, PINNED_CHAT_TEMPLATE_DIGEST,
    PINNED_TOKENIZER_DIGEST, WINDOW_PLUS_ONE_CORPUS_DIGEST,
};

/// A zeroizing production transport credential with a deliberately opaque
/// diagnostic surface.
pub struct ProtectedCredential {
    key: ProviderApiKey,
    leak_fingerprints: Vec<LeakFingerprint>,
}

impl ProtectedCredential {
    /// Takes ownership of one protected environment value.
    ///
    /// # Errors
    ///
    /// Rejects an empty credential before any request can be assembled.
    pub fn new(plaintext: String) -> Result<Self, QualificationError> {
        if plaintext.trim().is_empty() {
            return Err(QualificationError::EmptyCredential);
        }
        let mut leak_fingerprints = vec![LeakFingerprint::of(plaintext.as_bytes())];
        for encoded in [
            base64::engine::general_purpose::STANDARD.encode(plaintext.as_bytes()),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(plaintext.as_bytes()),
        ] {
            let fingerprint = LeakFingerprint::of(encoded.as_bytes());
            if !leak_fingerprints.contains(&fingerprint) {
                leak_fingerprints.push(fingerprint);
            }
        }
        Ok(Self {
            key: ProviderApiKey::new(plaintext),
            leak_fingerprints,
        })
    }

    pub(crate) const fn transport_key(&self) -> &ProviderApiKey {
        &self.key
    }

    fn leak_scanner(&self) -> LeakScanner {
        LeakScanner::new(self.leak_fingerprints.clone())
    }
}

impl fmt::Debug for ProtectedCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtectedCredential")
            .field("material", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LeakFingerprint {
    length: usize,
    digest: [u8; 32],
}

impl LeakFingerprint {
    fn of(bytes: &[u8]) -> Self {
        Self {
            length: bytes.len(),
            digest: Sha256::digest(bytes).into(),
        }
    }
}

#[derive(Debug)]
struct LeakScanner {
    fingerprints: Vec<LeakFingerprint>,
    tail: Vec<u8>,
    artifacts: u32,
    matches: u32,
}

impl LeakScanner {
    fn new(fingerprints: Vec<LeakFingerprint>) -> Self {
        Self {
            fingerprints,
            tail: Vec::new(),
            artifacts: 0,
            matches: 0,
        }
    }

    fn scan(&mut self, bytes: &[u8]) {
        self.artifacts = self.artifacts.saturating_add(1);
        let mut joined = core::mem::take(&mut self.tail);
        joined.extend_from_slice(bytes);
        for fingerprint in &self.fingerprints {
            if fingerprint.length == 0 || fingerprint.length > joined.len() {
                continue;
            }
            self.matches = self.matches.saturating_add(
                joined
                    .windows(fingerprint.length)
                    .filter(|window| <[u8; 32]>::from(Sha256::digest(window)) == fingerprint.digest)
                    .count()
                    .try_into()
                    .unwrap_or(u32::MAX),
            );
        }
        let keep = self
            .fingerprints
            .iter()
            .map(|fingerprint| fingerprint.length.saturating_sub(1))
            .max()
            .unwrap_or(0)
            .min(joined.len());
        self.tail.extend_from_slice(&joined[joined.len() - keep..]);
    }
}

/// Per-run negative inputs which cannot collide with a reviewed model id.
pub struct NegativeInputs {
    unknown_model: ModelSlug,
    invalid_credential: ProviderApiKey,
}

impl NegativeInputs {
    /// Derives negative values from GitHub's non-secret run identity.
    ///
    /// The source run id is hashed before it becomes a model slug, so neither
    /// a raw operational identity nor credential-shaped input is logged.
    ///
    /// # Errors
    ///
    /// Rejects a non-numeric run id, a zero attempt, or a derived slug that no
    /// longer fits the closed model-slug grammar.
    pub fn for_run(run_id: &str, attempt: u32) -> Result<Self, QualificationError> {
        if run_id.is_empty() || !run_id.bytes().all(|byte| byte.is_ascii_digit()) || attempt == 0 {
            return Err(QualificationError::InvalidRunIdentity);
        }
        let digest = Sha256::digest(format!("aex-model-catalog-negative-v1:{run_id}:{attempt}"));
        let hex = hex::encode(digest);
        let unknown_model = ModelSlug::new(format!("aex-never-{}", &hex[..24]))
            .map_err(|_| QualificationError::InvalidRunIdentity)?;
        let invalid_credential = ProviderApiKey::new(format!("aex-invalid-{}", &hex[24..56]));
        Ok(Self {
            unknown_model,
            invalid_credential,
        })
    }

    /// The guaranteed-unknown provider model slug used by P-20.
    #[must_use]
    pub fn unknown_model(&self) -> &str {
        self.unknown_model.as_str()
    }

    pub(crate) const fn invalid_credential(&self) -> &ProviderApiKey {
        &self.invalid_credential
    }
}

impl fmt::Debug for NegativeInputs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NegativeInputs")
            .field("unknown_model", &"[derived]")
            .field("invalid_credential", &"[redacted]")
            .finish()
    }
}

/// One explicit P-01--P-23 program bound to the exact reviewed target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSeekProgram {
    probe: ProbeId,
    target: QualificationTarget,
}

impl ProbeProgram for DeepSeekProgram {
    fn probe(&self) -> ProbeId {
        self.probe
    }

    fn target(&self) -> &QualificationTarget {
        &self.target
    }
}

/// Builds the exact reviewed target.
///
/// # Errors
///
/// Returns the closed readiness error if the shared candidate constants no
/// longer satisfy the provider qualification profile.
pub fn target() -> Result<QualificationTarget, ReadinessError> {
    QualificationTarget::new(
        ProviderId::Deepseek,
        DEEPSEEK_FLASH_MODEL,
        GENESIS_CAPABILITIES,
    )
}

/// Returns one explicit program for every registry probe.
///
/// # Errors
///
/// Propagates target readiness failure before any program is created.
pub fn programs() -> Result<Vec<DeepSeekProgram>, ReadinessError> {
    let target = target()?;
    Ok(ProbeId::ALL
        .into_iter()
        .map(|probe| DeepSeekProgram {
            probe,
            target: target.clone(),
        })
        .collect())
}

/// Required probes whose positive-provider implementation is not yet present.
///
/// This is checked before credential acquisition or network I/O. Keeping this
/// list derived from the capability matrix makes a newly unconditional probe a
/// hard failure without a hand-maintained allow-list update.
#[must_use]
pub fn unsupported_required_probes() -> Vec<ProbeId> {
    ProbeId::ALL
        .into_iter()
        .filter(|probe| probe.is_required_for(GENESIS_CAPABILITIES))
        .filter(|probe| !is_implemented(*probe))
        .collect()
}

const fn is_implemented(probe: ProbeId) -> bool {
    matches!(
        probe,
        ProbeId::P01
            | ProbeId::P02
            | ProbeId::P11
            | ProbeId::P13
            | ProbeId::P15
            | ProbeId::P16
            | ProbeId::P17
            | ProbeId::P18
            | ProbeId::P19
            | ProbeId::P20
            | ProbeId::P21
            | ProbeId::P22
            | ProbeId::P23
    )
}

/// Refuses to manufacture live evidence from local canned fault shapes.
///
/// Fault evidence must be produced asynchronously by the production send and
/// stream seams; this synchronous compatibility surface never manufactures it.
#[must_use]
pub fn fault_evidence(probe: ProbeId) -> Option<ProbeEvidence> {
    let _ = probe;
    None
}

const DEEPSEEK_PROGRESS_FRAME: &[u8] = concat!(
    "data: {\"id\":\"qualification\",\"object\":\"chat.completion.chunk\",",
    "\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"A\"},",
    "\"finish_reason\":null}],\"usage\":null}\n\n",
)
.as_bytes();

const DEEPSEEK_TERMINAL_STREAM: &[u8] = concat!(
    "data: {\"id\":\"qualification\",\"object\":\"chat.completion.chunk\",",
    "\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"A\"},",
    "\"finish_reason\":\"stop\"}],\"usage\":null}\n\n",
    "data: {\"id\":\"qualification\",\"object\":\"chat.completion.chunk\",",
    "\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"prompt_cache_hit_tokens\":0,",
    "\"prompt_cache_miss_tokens\":1,\"completion_tokens\":1,",
    "\"completion_tokens_details\":{\"reasoning_tokens\":0},\"total_tokens\":2}}\n\n",
    "data: [DONE]\n\n",
)
.as_bytes();

#[derive(Debug, Clone)]
struct CancelOnResponseStart(CancelToken);

impl ResponseStartSink for CancelOnResponseStart {
    fn mark(
        &self,
        _provider_request_id: Option<ProviderRequestId>,
    ) -> BoxFuture<'_, Result<(), ResponseStartSinkError>> {
        self.0.cancel();
        Box::pin(async { Ok(()) })
    }
}

/// Driver over the production `DeepSeek` transport and classifier.
/// Unsupported fault and exact-token programs never enter this type.
pub struct DeepSeekQualificationDriver {
    client: reqwest::Client,
    entry: ModelEntry,
    credential: ProtectedCredential,
    negative: NegativeInputs,
    corpora: DeepSeekCorpusPair,
    budget: StreamBudget,
    usage: Arc<Mutex<NormalizedUsage>>,
    leak_scanner: Arc<Mutex<LeakScanner>>,
}

impl DeepSeekQualificationDriver {
    /// Builds one bounded client. Redirects are disabled so the compiled
    /// provider origin cannot hand a credential to another host.
    ///
    /// # Errors
    ///
    /// Rejects a mismatched candidate or a client whose closed timeout and
    /// redirect policy cannot be configured.
    pub fn new(
        entry: ModelEntry,
        credential: ProtectedCredential,
        negative: NegativeInputs,
        corpora: DeepSeekCorpusPair,
        usage: Arc<Mutex<NormalizedUsage>>,
    ) -> Result<Self, QualificationRunError> {
        validate_candidate(&entry)?;
        validate_corpora(&entry, &corpora)?;
        let budget = budget_for(&entry);
        let leak_scanner = Arc::new(Mutex::new(credential.leak_scanner()));
        let client = reqwest::Client::builder()
            .connect_timeout(budget.connect_timeout)
            .timeout(budget.total_deadline)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| QualificationRunError::ClientConfiguration)?;
        Ok(Self {
            client,
            entry,
            credential,
            negative,
            corpora,
            budget,
            usage,
            leak_scanner,
        })
    }

    fn request(&self, prompt: String, maximum_output: u32) -> Result<WireRequest, RedactedDetail> {
        let message = qualification_message(prompt)?;
        build_qualification_request(
            &self.entry,
            qualification_request(core::slice::from_ref(&message), maximum_output),
        )
        .map_err(|_| invalid_program("the production qualification request was rejected"))
    }

    fn request_message(
        &self,
        message: &CanonicalMessage,
        maximum_output: u32,
    ) -> Result<WireRequest, RedactedDetail> {
        build_qualification_request(
            &self.entry,
            qualification_request(core::slice::from_ref(message), maximum_output),
        )
        .map_err(|_| invalid_program("the exact oracle message was rejected"))
    }

    fn non_stream_request(
        &self,
        prompt: String,
        maximum_output: u32,
    ) -> Result<WireRequest, RedactedDetail> {
        let message = qualification_message(prompt)?;
        build_non_streamed_qualification_request(
            &self.entry,
            qualification_request(core::slice::from_ref(&message), maximum_output),
        )
        .map_err(|_| invalid_program("the production non-stream parity request was rejected"))
    }

    fn spend(&self, usage: NormalizedUsage) {
        if let Ok(mut aggregate) = self.usage.lock() {
            aggregate.add(usage);
        }
    }

    async fn minimal_text(&mut self) -> Result<ProbeEvidence, RedactedDetail> {
        let prompt = "Reply with exactly AEX_OK and no other text.".to_owned();
        let streamed = self
            .dispatch_stream(self.request(prompt.clone(), 64)?)
            .await
            .map_err(LiveFailure::into_detail)?;
        let non_streamed = self
            .dispatch_non_stream(self.non_stream_request(prompt, 64)?)
            .await
            .map_err(LiveFailure::into_detail)?;
        self.spend(streamed.sealed.usage);
        self.spend(non_streamed.sealed.usage);
        Ok(ProbeEvidence::MinimalText {
            streamed_shape: response_shape(&streamed.sealed),
            non_streamed_shape: response_shape(&non_streamed.sealed),
            text_bytes: text_bytes(&streamed.sealed),
        })
    }

    async fn long_stream(&mut self) -> Result<ProbeEvidence, RedactedDetail> {
        let prompt = "Output the decimal integers 000000 through 002047 in order, exactly one six-digit integer per line, with no heading, code fence, omission, or other text.".to_owned();
        let observation = self
            .dispatch_stream(self.request(prompt, bounded_output(&self.entry, 16_384))?)
            .await
            .map_err(LiveFailure::into_detail)?;
        self.spend(observation.sealed.usage);
        let text = response_text(&observation.sealed);
        let sequence_valid = ordered_sequence(&text, 2_048);
        Ok(ProbeEvidence::LongStream {
            output_bytes: u64::try_from(text.len()).unwrap_or(u64::MAX),
            frames: observation.frames,
            ordered: sequence_valid,
            lossless: sequence_valid,
        })
    }

    async fn usage_parity(&mut self) -> Result<ProbeEvidence, RedactedDetail> {
        let prompt = "Reply with exactly USAGE_OK and no other text.".to_owned();
        let streamed = self
            .dispatch_stream(self.request(prompt.clone(), 64)?)
            .await
            .map_err(LiveFailure::into_detail)?;
        let non_streamed = self
            .dispatch_non_stream(self.non_stream_request(prompt, 64)?)
            .await
            .map_err(LiveFailure::into_detail)?;
        let stream_usage = streamed.sealed.usage;
        let non_stream_usage = non_streamed.sealed.usage;
        self.spend(stream_usage);
        self.spend(non_stream_usage);
        Ok(ProbeEvidence::Usage {
            stream_complete: stream_usage.completeness == UsageCompleteness::Exact,
            non_stream_complete: non_stream_usage.completeness == UsageCompleteness::Exact,
            mapping_matches: usage_mapping_matches(stream_usage)
                && usage_mapping_matches(non_stream_usage),
        })
    }

    async fn output_ceiling(&mut self) -> Result<ProbeEvidence, RedactedDetail> {
        let observation = self
            .dispatch_stream(self.request(
                "Write an unending sequence of decimal integers without stopping.".to_owned(),
                self.entry.limits.min_output_tokens.max(1),
            )?)
            .await
            .map_err(LiveFailure::into_detail)?;
        self.spend(observation.sealed.usage);
        Ok(ProbeEvidence::OutputCeiling {
            stop_reason: observation.sealed.stop_reason,
        })
    }

    async fn long_context(&mut self) -> Result<ProbeEvidence, RedactedDetail> {
        let case = &self.corpora.eighty_percent;
        let request =
            self.request_message(&case.message, self.entry.limits.min_output_tokens.max(1))?;
        let observation = self
            .dispatch_stream(request)
            .await
            .map_err(LiveFailure::into_detail)?;
        let usage = observation.sealed.usage;
        let provider_prompt_tokens =
            (usage.completeness == UsageCompleteness::Exact).then_some(usage.prompt_tokens());
        self.spend(usage);
        Ok(ProbeEvidence::LongContext {
            oracle_prompt_tokens: case.exact_chat_tokens,
            provider_prompt_tokens,
            context_window_tokens: self.corpora.context_window_tokens,
            completed: true,
            tokenizer_digest: self.corpora.tokenizer_digest,
            chat_template_digest: self.corpora.chat_template_digest,
            corpus_digest: case.corpus_digest,
        })
    }

    async fn context_overflow(&self) -> Result<ProbeEvidence, RedactedDetail> {
        let case = &self.corpora.window_plus_one;
        let request =
            self.request_message(&case.message, self.entry.limits.min_output_tokens.max(1))?;
        let response = send_head(
            &self.client,
            &request,
            self.credential.transport_key(),
            self.budget.head_timeout,
        )
        .await
        .map_err(LiveFailure::into_detail)?;
        let (kind, truncated) = if response.status().is_success() {
            scan_headers(&self.leak_scanner, response.headers());
            (ProviderFailureKind::ProtocolViolation, true)
        } else {
            (
                classify_failure(response, &self.budget, &self.leak_scanner)
                    .await
                    .kind,
                false,
            )
        };
        Ok(ProbeEvidence::ContextOverflow {
            attempted_prompt_tokens: case.exact_chat_tokens,
            context_window_tokens: self.corpora.context_window_tokens,
            kind,
            truncated,
            tokenizer_digest: self.corpora.tokenizer_digest,
            chat_template_digest: self.corpora.chat_template_digest,
            corpus_digest: case.corpus_digest,
        })
    }

    async fn rate_limit(&mut self) -> Result<ProbeEvidence, RedactedDetail> {
        let observation = self
            .dispatch_stream(self.request(
                "Reply with exactly RATE_OK and no other text.".to_owned(),
                bounded_output(&self.entry, 8),
            )?)
            .await
            .map_err(LiveFailure::into_detail)?;
        self.spend(observation.sealed.usage);
        Ok(rate_limit_evidence(&observation.rate_limit))
    }

    async fn error_taxonomy(&mut self) -> Result<ProbeEvidence, RedactedDetail> {
        let invalid = self.request(
            "Reply with exactly NEGATIVE_OK and no other text.".to_owned(),
            self.entry.limits.min_output_tokens.max(1),
        )?;
        let wrong_key = classify_negative(
            &self.client,
            &invalid,
            self.negative.invalid_credential(),
            &self.leak_scanner,
        )
        .await?;

        let mut malformed = invalid.clone();
        malformed.body = Bytes::from_static(b"{");
        let malformed_body = classify_negative(
            &self.client,
            &malformed,
            self.credential.transport_key(),
            &self.leak_scanner,
        )
        .await?;

        let mut unknown_entry = self.entry.clone();
        unknown_entry.model = ModelSlug::new(self.negative.unknown_model())
            .map_err(|_| invalid_program("the derived unknown model slug is invalid"))?;
        let message = qualification_message(
            "Reply with exactly UNKNOWN_UNEXPECTED and no other text.".to_owned(),
        )?;
        let unknown = build_qualification_request(
            &unknown_entry,
            qualification_request(
                core::slice::from_ref(&message),
                unknown_entry.limits.min_output_tokens.max(1),
            ),
        )
        .map_err(|_| invalid_program("the unknown-model negative request was rejected locally"))?;
        let unknown_model = classify_negative(
            &self.client,
            &unknown,
            self.credential.transport_key(),
            &self.leak_scanner,
        )
        .await?;

        Ok(ProbeEvidence::ErrorTaxonomy {
            wrong_key: wrong_key.kind,
            malformed_body: malformed_body.kind,
            unknown_model: unknown_model.kind,
            body_shapes: [wrong_key.shape, malformed_body.shape, unknown_model.shape],
        })
    }

    async fn credential_leak(&mut self) -> Result<ProbeEvidence, RedactedDetail> {
        let observation = self
            .dispatch_stream(self.request(
                "Reply with exactly LEAK_OK and no other text.".to_owned(),
                bounded_output(&self.entry, 8),
            )?)
            .await
            .map_err(LiveFailure::into_detail)?;
        self.spend(observation.sealed.usage);
        let (artifacts_scanned, matches) =
            self.leak_scanner.lock().map_or((0, u32::MAX), |scanner| {
                (scanner.artifacts, scanner.matches)
            });
        Ok(ProbeEvidence::CredentialLeak {
            artifacts_scanned,
            matches,
        })
    }

    async fn cancellation(&self) -> Result<ProbeEvidence, RedactedDetail> {
        let response = self
            .open_stream(self.request(
                "Output the letter A repeatedly until stopped.".to_owned(),
                bounded_output(&self.entry, 8_192),
            )?)
            .await?;
        let headers = response.headers().clone();
        let scanner = Arc::clone(&self.leak_scanner);
        let body = response.bytes_stream().map(move |chunk| {
            if let Ok(bytes) = &chunk {
                scan_artifact(&scanner, bytes);
            }
            chunk
        });
        let cancel = CancelToken::new();
        let sink = CancelOnResponseStart(cancel.clone());
        let result = consume_deepseek_qualification_stream(
            &self.entry,
            body,
            HeaderView::new(&headers),
            &self.budget,
            &NullPreviewSink,
            &cancel,
            Instant::now(),
            &sink,
        )
        .await;
        let error = match result {
            Err(QualificationStreamError::Consume(error))
                if matches!(error.failure, StreamFailure::Cancelled) =>
            {
                error
            }
            Err(error) => return Err(qualification_stream_detail(error)),
            Ok(_) => {
                return Err(invalid_program(
                    "the production stream ignored response-start cancellation",
                ));
            }
        };
        Ok(ProbeEvidence::Cancellation {
            kind: error.kind(),
            response_bytes: error.response_bytes,
            frames_after_cancel: error.frames_after_cancel,
        })
    }

    async fn forced_drops(&self) -> Result<ProbeEvidence, RedactedDetail> {
        let request = self.request(
            "Reply with exactly DROP_OK and no other text.".to_owned(),
            bounded_output(&self.entry, 8),
        )?;

        let mut send = SendState::new();
        let pre_headers = match execute_qualification_fault(
            &self.client,
            &request,
            self.credential.transport_key(),
            &mut send,
            QualificationSendFault::DropBeforeNetwork,
        )
        .await
        {
            Err(error) => error.proof(),
            Ok(_) => {
                return Err(invalid_program(
                    "the production send-gate fault did not fire",
                ));
            }
        };

        let headers = reqwest::header::HeaderMap::new();
        let pre_frame = consume_deepseek_qualification_stream(
            &self.entry,
            stream::iter([Err::<Bytes, ()>(())]),
            HeaderView::new(&headers),
            &self.budget,
            &NullPreviewSink,
            &CancelToken::new(),
            Instant::now(),
            &NullResponseStartSink,
        )
        .await;
        let post_headers_pre_frame = stream_drop_proof(pre_frame)?;

        let mid_frame = consume_deepseek_qualification_stream(
            &self.entry,
            stream::iter([
                Ok::<Bytes, ()>(Bytes::from_static(DEEPSEEK_PROGRESS_FRAME)),
                Ok(Bytes::from_static(b"data: {")),
                Err(()),
            ]),
            HeaderView::new(&headers),
            &self.budget,
            &NullPreviewSink,
            &CancelToken::new(),
            Instant::now(),
            &NullResponseStartSink,
        )
        .await;
        let mid_frame = stream_drop_proof(mid_frame)?;

        let post_terminal_completed = consume_deepseek_qualification_stream(
            &self.entry,
            stream::iter([
                Ok::<Bytes, ()>(Bytes::from_static(DEEPSEEK_TERMINAL_STREAM)),
                Err(()),
            ]),
            HeaderView::new(&headers),
            &self.budget,
            &NullPreviewSink,
            &CancelToken::new(),
            Instant::now(),
            &NullResponseStartSink,
        )
        .await
        .is_ok();

        Ok(ProbeEvidence::ForcedDrops {
            pre_headers,
            post_headers_pre_frame,
            mid_frame,
            post_terminal_completed,
        })
    }

    async fn oversized_frame(&self) -> Result<ProbeEvidence, RedactedDetail> {
        let headers = reqwest::header::HeaderMap::new();
        let mut budget = budget_for(&self.entry);
        budget.max_frame_bytes = 8;
        let result = consume_deepseek_qualification_stream(
            &self.entry,
            stream::iter([Ok::<Bytes, ()>(Bytes::from_static(
                b"data: frame-is-too-large\n\n",
            ))]),
            HeaderView::new(&headers),
            &budget,
            &NullPreviewSink,
            &CancelToken::new(),
            Instant::now(),
            &NullResponseStartSink,
        )
        .await;
        let error = stream_consume_error(result)?;
        Ok(ProbeEvidence::OversizedFrame {
            kind: error.kind(),
            proof: error.proof(),
        })
    }

    async fn idle_timeout(&self) -> Result<ProbeEvidence, RedactedDetail> {
        let headers = reqwest::header::HeaderMap::new();
        let mut budget = budget_for(&self.entry);
        budget.idle_frame_timeout = Duration::from_millis(25);
        budget.total_deadline = Duration::from_millis(250);
        let started = Instant::now();
        let result = tokio::time::timeout(
            budget
                .total_deadline
                .saturating_add(Duration::from_millis(250)),
            consume_deepseek_qualification_stream(
                &self.entry,
                stream::iter([Ok::<Bytes, ()>(Bytes::from_static(DEEPSEEK_PROGRESS_FRAME))])
                    .chain(stream::pending()),
                HeaderView::new(&headers),
                &budget,
                &NullPreviewSink,
                &CancelToken::new(),
                started,
                &NullResponseStartSink,
            ),
        )
        .await;
        let observed = started.elapsed();
        let hung = result.is_err();
        let result = result.map_err(|_| {
            invalid_program("the production idle schedule escaped its enclosing deadline")
        })?;
        let error = stream_consume_error(result)?;
        let StreamFailure::Budget(BudgetOverrun::IdleFrame { after }) = &error.failure else {
            return Err(invalid_program(
                "the idle schedule did not produce the production idle-frame failure",
            ));
        };
        Ok(ProbeEvidence::IdleTimeout {
            kind: error.kind(),
            idle_limit_ms: duration_millis(*after),
            observed_after_ms: duration_millis(observed),
            hung,
        })
    }

    async fn open_stream(&self, request: WireRequest) -> Result<reqwest::Response, RedactedDetail> {
        let response = send_head(
            &self.client,
            &request,
            self.credential.transport_key(),
            self.budget.head_timeout,
        )
        .await
        .map_err(LiveFailure::into_detail)?;
        if !response.status().is_success() {
            return Err(classify_failure(response, &self.budget, &self.leak_scanner)
                .await
                .into_detail());
        }
        scan_headers(&self.leak_scanner, response.headers());
        Ok(response)
    }

    async fn dispatch_stream(
        &self,
        request: WireRequest,
    ) -> Result<StreamObservation, LiveFailure> {
        let response = self
            .open_stream(request)
            .await
            .map_err(|detail| LiveFailure::new(detail.kind, "the live stream could not open"))?;
        let headers = response.headers().clone();
        let rate_limit = DeepSeekAdapter.rate_limit_feedback(&HeaderView::new(&headers));
        let scanner = Arc::clone(&self.leak_scanner);
        let body = response.bytes_stream().map(move |chunk| {
            if let Ok(bytes) = &chunk {
                scan_artifact(&scanner, bytes);
            }
            chunk
        });
        let consumed = consume_deepseek_qualification_stream(
            &self.entry,
            body,
            HeaderView::new(&headers),
            &self.budget,
            &NullPreviewSink,
            &CancelToken::new(),
            Instant::now(),
            &NullResponseStartSink,
        )
        .await
        .map_err(qualification_stream_failure)?;
        Ok(StreamObservation {
            sealed: consumed.response,
            frames: consumed.frames,
            rate_limit,
        })
    }

    async fn dispatch_non_stream(
        &self,
        request: WireRequest,
    ) -> Result<StreamObservation, LiveFailure> {
        let mut response = send_head(
            &self.client,
            &request,
            self.credential.transport_key(),
            self.budget.head_timeout,
        )
        .await?;
        if !response.status().is_success() {
            return Err(classify_failure(response, &self.budget, &self.leak_scanner).await);
        }
        let headers = response.headers().clone();
        let rate_limit = DeepSeekAdapter.rate_limit_feedback(&HeaderView::new(&headers));
        scan_headers(&self.leak_scanner, &headers);
        let body = read_bounded_response(
            &mut response,
            self.budget.max_response_bytes,
            &self.leak_scanner,
        )
        .await?;
        let sealed =
            decode_non_streamed_qualification_response(&body, &self.budget).map_err(|_| {
                LiveFailure::protocol("the production decoder rejected the non-streamed response")
            })?;
        Ok(StreamObservation {
            sealed,
            frames: 1,
            rate_limit,
        })
    }
}

impl ProbeDriver<DeepSeekProgram> for DeepSeekQualificationDriver {
    fn execute<'a>(&'a mut self, program: &'a DeepSeekProgram) -> ProbeDriverFuture<'a> {
        Box::pin(async move {
            if let Some(evidence) = fault_evidence(program.probe()) {
                return Ok(evidence);
            }
            match program.probe() {
                ProbeId::P01 => self.minimal_text().await,
                ProbeId::P02 => self.long_stream().await,
                ProbeId::P11 => self.usage_parity().await,
                ProbeId::P13 => self.output_ceiling().await,
                ProbeId::P15 => self.cancellation().await,
                ProbeId::P16 => self.long_context().await,
                ProbeId::P17 => self.context_overflow().await,
                ProbeId::P18 => self.forced_drops().await,
                ProbeId::P19 => self.rate_limit().await,
                ProbeId::P20 => self.error_taxonomy().await,
                ProbeId::P21 => self.credential_leak().await,
                ProbeId::P22 => self.oversized_frame().await,
                ProbeId::P23 => self.idle_timeout().await,
                _ => Err(invalid_program(
                    "the capability-gated qualification program is not applicable",
                )),
            }
        })
    }
}

/// Result of one complete protected qualification matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualificationReport {
    /// All P-01--P-23 rows, including explicit not-applicable rows.
    pub runs: Vec<ProbeRun>,
    /// Provider-reported usage accumulated from every successfully sealed call.
    pub tokens_spent: NormalizedUsage,
    /// Deterministic worst-case cost admitted before the first provider call.
    pub maximum_cost_micro_usd: u64,
}

/// Deterministic worst-case spend for the exact required program inventory.
///
/// `DeepSeek` publishes `$0.14` per million cache-miss input tokens and `$0.28`
/// per million output tokens for this target. The bound pessimistically charges
/// every request a full context window, ignores cache discounts, and charges
/// every configured output ceiling.
///
/// # Errors
///
/// Rejects anything other than the exact shared staged candidate.
pub fn maximum_matrix_cost_micro_usd(entry: &ModelEntry) -> Result<u64, QualificationRunError> {
    const PROVIDER_CALLS: u64 = 14;
    validate_candidate(entry)?;
    let input_tokens = u64::from(entry.limits.context_window_tokens).saturating_mul(PROVIDER_CALLS);
    let output_tokens = u64::from(bounded_output(entry, 64)).saturating_mul(4)
        + u64::from(bounded_output(entry, 16_384))
        + u64::from(entry.limits.min_output_tokens.max(1)).saturating_mul(6)
        + u64::from(bounded_output(entry, 8)).saturating_mul(2)
        + u64::from(bounded_output(entry, 8_192));
    Ok(rate_ceiling(input_tokens, 14).saturating_add(rate_ceiling(output_tokens, 28)))
}

/// Runs the exact matrix after completeness, candidate and worst-case cost
/// checks have all passed.
///
/// # Errors
///
/// Fails before client construction when any required program is unavailable,
/// the candidate differs, or operator budget is insufficient. Otherwise it
/// propagates closed readiness or redacted driver failures.
pub async fn execute_matrix(
    credential_variable: &'static str,
    entry: ModelEntry,
    credential: ProtectedCredential,
    negative: NegativeInputs,
    corpora: DeepSeekCorpusPair,
    maximum_budget_micro_usd: u64,
) -> Result<QualificationReport, QualificationRunError> {
    let missing = unsupported_required_probes();
    if !missing.is_empty() {
        return Err(QualificationRunError::IncompletePrograms { probes: missing });
    }
    let maximum_cost_micro_usd = maximum_matrix_cost_micro_usd(&entry)?;
    if maximum_cost_micro_usd > maximum_budget_micro_usd {
        return Err(QualificationRunError::BudgetInsufficient {
            required_micro_usd: maximum_cost_micro_usd,
            approved_micro_usd: maximum_budget_micro_usd,
        });
    }
    let target = target()?;
    let matrix =
        prepare_with_credential_presence(target.clone(), |name| name == credential_variable)?;
    let usage = Arc::new(Mutex::new(NormalizedUsage::default()));
    let driver =
        DeepSeekQualificationDriver::new(entry, credential, negative, corpora, Arc::clone(&usage))?;
    let mut executor = ConfiguredProbeExecutor::new(target, programs()?, driver)?;
    let runs = matrix.execute(&mut executor).await?;
    // The matrix deliberately contains cancelled, rejected and injected-fault
    // calls for which no complete provider usage can exist. Exact usage from
    // successful rows cannot be represented as exact whole-matrix spend.
    let tokens_spent = incomplete_matrix_usage(&usage);
    Ok(QualificationReport {
        runs,
        tokens_spent,
        maximum_cost_micro_usd,
    })
}

fn incomplete_matrix_usage(usage: &Mutex<NormalizedUsage>) -> NormalizedUsage {
    let mut aggregate = usage
        .lock()
        .map_or_else(|_| NormalizedUsage::default(), |observed| *observed);
    aggregate.completeness = UsageCompleteness::Absent;
    aggregate.provider_total_tokens = None;
    aggregate
}

#[derive(Debug)]
struct ClassifiedNegative {
    kind: ProviderFailureKind,
    shape: ErrorBodyShape,
}

#[derive(Debug)]
struct StreamObservation {
    sealed: SealedResponse,
    frames: u32,
    rate_limit: RateLimitFeedback,
}

#[derive(Debug)]
struct LiveFailure {
    kind: ProviderFailureKind,
    label: &'static str,
}

impl LiveFailure {
    const fn new(kind: ProviderFailureKind, label: &'static str) -> Self {
        Self { kind, label }
    }

    const fn timeout(label: &'static str) -> Self {
        Self::new(ProviderFailureKind::Timeout, label)
    }

    const fn transport(label: &'static str) -> Self {
        Self::new(ProviderFailureKind::Transport, label)
    }

    const fn protocol(label: &'static str) -> Self {
        Self::new(ProviderFailureKind::ProtocolViolation, label)
    }

    fn into_detail(self) -> RedactedDetail {
        RedactedDetail::internal(self.kind, self.label)
    }
}

fn qualification_stream_failure(error: QualificationStreamError) -> LiveFailure {
    match error {
        QualificationStreamError::Candidate => LiveFailure::protocol(
            "the production consumer refused the reviewed staged DeepSeek candidate",
        ),
        QualificationStreamError::Consume(error) => LiveFailure::new(
            error.kind(),
            "the production stream consumer could not seal the response",
        ),
    }
}

fn qualification_stream_detail(error: QualificationStreamError) -> RedactedDetail {
    qualification_stream_failure(error).into_detail()
}

fn stream_consume_error(
    result: Result<ConsumedStream, QualificationStreamError>,
) -> Result<StreamConsumeError, RedactedDetail> {
    match result {
        Err(QualificationStreamError::Consume(error)) => Ok(error),
        Err(error) => Err(qualification_stream_detail(error)),
        Ok(_) => Err(invalid_program(
            "the production fault schedule unexpectedly completed",
        )),
    }
}

fn stream_drop_proof(
    result: Result<ConsumedStream, QualificationStreamError>,
) -> Result<DispatchProof, RedactedDetail> {
    Ok(stream_consume_error(result)?.proof())
}

fn duration_millis(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

static AUTO_TOOL_CHOICE: ToolChoice = ToolChoice::Auto;

fn qualification_request(
    messages: &[CanonicalMessage],
    maximum_output: u32,
) -> QualificationRequest<'_> {
    QualificationRequest {
        system: &[],
        messages,
        tools: &[],
        tool_choice: &AUTO_TOOL_CHOICE,
        parallel_tools: false,
        max_output_tokens: maximum_output,
        temperature_milli: None,
        top_p_milli: None,
        stop_sequences: &[],
        reasoning: ReasoningRequest::Disabled,
        structured_output: None,
    }
}

fn qualification_message(prompt: String) -> Result<CanonicalMessage, RedactedDetail> {
    let text = BoundedString::new(prompt)
        .map_err(|_| invalid_program("the qualification prompt exceeded its closed bound"))?;
    Ok(CanonicalMessage {
        role: Role::User,
        blocks: vec![CanonicalBlock::Text {
            text,
            annotations: Vec::new(),
        }],
    })
}

fn invalid_program(label: &'static str) -> RedactedDetail {
    RedactedDetail::internal(ProviderFailureKind::InvalidRequest, label)
}

fn bounded_output(entry: &ModelEntry, preferred: u32) -> u32 {
    preferred
        .max(entry.limits.min_output_tokens)
        .min(entry.limits.max_output_tokens)
}

fn ordered_sequence(text: &str, expected: u32) -> bool {
    let lines: Vec<&str> = text.trim().lines().collect();
    lines.len() == usize::try_from(expected).unwrap_or(usize::MAX)
        && lines.iter().enumerate().all(|(index, line)| {
            let Ok(index) = u32::try_from(index) else {
                return false;
            };
            *line == format!("{index:06}")
        })
}

fn response_text(response: &SealedResponse) -> String {
    response
        .blocks
        .iter()
        .filter_map(|block| match block {
            CanonicalBlock::Text { text, .. } => Some(text.as_str()),
            CanonicalBlock::Reasoning(_)
            | CanonicalBlock::ToolUse { .. }
            | CanonicalBlock::ToolResult { .. }
            | CanonicalBlock::Refusal { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn text_bytes(response: &SealedResponse) -> u64 {
    u64::try_from(response_text(response).len()).unwrap_or(u64::MAX)
}

fn response_shape(response: &SealedResponse) -> ContentHash {
    let bytes = serde_json::to_vec(&(&response.blocks, response.stop_reason))
        .expect("closed canonical response members serialize");
    ContentHash::of(&bytes)
}

fn usage_mapping_matches(usage: NormalizedUsage) -> bool {
    if usage.completeness != UsageCompleteness::Exact || !usage.is_consistent() {
        return false;
    }
    usage
        .provider_total_tokens
        .is_some_and(|total| total == usage.prompt_tokens().saturating_add(usage.output_tokens))
}

fn rate_limit_evidence(feedback: &RateLimitFeedback) -> ProbeEvidence {
    let observed = feedback.retry_after.is_some()
        || feedback.requests_remaining.is_some()
        || feedback.tokens_remaining.is_some()
        || feedback.reset_at.is_some();
    ProbeEvidence::RateLimit {
        source: feedback.source,
        feedback_observed: observed,
        absence_recorded: feedback.is_absent() && !observed,
    }
}

fn budget_for(entry: &ModelEntry) -> StreamBudget {
    let mut budget = StreamBudget::default();
    budget.max_frame_bytes = budget
        .max_frame_bytes
        .min(entry.limits.response_frame_max_bytes);
    budget.idle_frame_timeout = budget
        .idle_frame_timeout
        .min(Duration::from_millis(u64::from(
            entry.limits.stream_idle_timeout_ms,
        )));
    budget.total_deadline = budget.total_deadline.min(Duration::from_millis(u64::from(
        entry.limits.total_stream_deadline_ms,
    )));
    budget
}

fn validate_candidate(entry: &ModelEntry) -> Result<(), QualificationRunError> {
    if entry.provider != ProviderId::Deepseek
        || entry.model.as_str() != DEEPSEEK_FLASH_MODEL
        || entry.state != EntryState::Staged
        || entry.dialect != aex_model_catalog::document::Dialect::DeepSeekChat
        || entry.endpoint != EndpointPin::DeepSeekApi
        || entry.capabilities != GENESIS_CAPABILITIES
        || entry.limits.context_window_tokens == 0
        || entry.limits.max_output_tokens < entry.limits.min_output_tokens
    {
        return Err(QualificationRunError::CandidateMismatch);
    }
    Ok(())
}

fn validate_corpora(
    entry: &ModelEntry,
    corpora: &DeepSeekCorpusPair,
) -> Result<(), QualificationRunError> {
    let eighty_percent = u64::from(entry.limits.context_window_tokens).saturating_mul(4) / 5;
    let plus_one = entry
        .limits
        .context_window_tokens
        .checked_add(1)
        .ok_or(QualificationRunError::CorpusMismatch)?;
    let eighty_digest = to_jcs_bytes(&corpora.eighty_percent.message)
        .map(|bytes| ContentHash::of(&bytes))
        .map_err(|_| QualificationRunError::CorpusMismatch)?;
    let plus_one_digest = to_jcs_bytes(&corpora.window_plus_one.message)
        .map(|bytes| ContentHash::of(&bytes))
        .map_err(|_| QualificationRunError::CorpusMismatch)?;
    if corpora.context_window_tokens != entry.limits.context_window_tokens
        || corpora.tokenizer_digest != PINNED_TOKENIZER_DIGEST
        || corpora.chat_template_digest != PINNED_CHAT_TEMPLATE_DIGEST
        || corpora.eighty_percent.target_chat_tokens != u32::try_from(eighty_percent).unwrap_or(0)
        || corpora.eighty_percent.exact_chat_tokens != corpora.eighty_percent.target_chat_tokens
        || corpora.eighty_percent.corpus_digest != EIGHTY_PERCENT_CORPUS_DIGEST
        || eighty_digest != corpora.eighty_percent.corpus_digest
        || corpora.window_plus_one.target_chat_tokens != plus_one
        || corpora.window_plus_one.exact_chat_tokens != plus_one
        || corpora.window_plus_one.corpus_digest != WINDOW_PLUS_ONE_CORPUS_DIGEST
        || plus_one_digest != corpora.window_plus_one.corpus_digest
    {
        return Err(QualificationRunError::CorpusMismatch);
    }
    Ok(())
}

fn rate_ceiling(tokens: u64, hundredths_micro_usd_per_token: u64) -> u64 {
    tokens
        .saturating_mul(hundredths_micro_usd_per_token)
        .saturating_add(99)
        / 100
}

async fn send_head(
    client: &reqwest::Client,
    request: &WireRequest,
    credential: &ProviderApiKey,
    head_timeout: Duration,
) -> Result<reqwest::Response, LiveFailure> {
    let mut state = SendState::new();
    tokio::time::timeout(
        head_timeout,
        execute(client, request, credential, &mut state),
    )
    .await
    .map_err(|_| LiveFailure::timeout("the provider response head deadline elapsed"))?
    .map_err(|error| {
        if error.proof() == DispatchProof::PossiblySent {
            LiveFailure::transport("the provider transport failed after dispatch")
        } else {
            LiveFailure::transport("the provider request was rejected before dispatch")
        }
    })
}

fn scan_artifact(scanner: &Arc<Mutex<LeakScanner>>, bytes: &[u8]) {
    if let Ok(mut scanner) = scanner.lock() {
        scanner.scan(bytes);
    }
}

fn scan_headers(scanner: &Arc<Mutex<LeakScanner>>, headers: &reqwest::header::HeaderMap) {
    for (name, value) in headers {
        scan_artifact(scanner, name.as_str().as_bytes());
        scan_artifact(scanner, value.as_bytes());
    }
}

async fn read_bounded_response(
    response: &mut reqwest::Response,
    limit: u64,
    scanner: &Arc<Mutex<LeakScanner>>,
) -> Result<BoundedBody, LiveFailure> {
    let capacity = usize::try_from(limit.min(64 * 1024)).unwrap_or(64 * 1024);
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| LiveFailure::transport("the provider response body could not be read"))?
    {
        scan_artifact(scanner, &chunk);
        if u64::try_from(bytes.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX))
            > limit
        {
            return Err(LiveFailure::protocol(
                "the provider response crossed its byte bound",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(BoundedBody::new(bytes, false))
}

async fn classify_failure(
    mut response: reqwest::Response,
    budget: &StreamBudget,
    scanner: &Arc<Mutex<LeakScanner>>,
) -> LiveFailure {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    scan_headers(scanner, &headers);
    let limit = usize::try_from(budget.max_error_body_bytes).unwrap_or(16 * 1024);
    let mut bytes = Vec::with_capacity(limit.min(16 * 1024));
    let mut truncated = false;
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return LiveFailure::transport("the provider error body could not be read"),
        };
        scan_artifact(scanner, &chunk);
        let remaining = limit.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = BoundedBody::new(bytes, truncated);
    let failure = DeepSeekAdapter.classify_http(status, &HeaderView::new(&headers), &body);
    LiveFailure::new(
        failure.kind(),
        "the provider rejected a qualification request",
    )
}

async fn classify_negative(
    client: &reqwest::Client,
    request: &WireRequest,
    credential: &ProviderApiKey,
    scanner: &Arc<Mutex<LeakScanner>>,
) -> Result<ClassifiedNegative, RedactedDetail> {
    const BODY_LIMIT: usize = 16 * 1024;
    let mut send = SendState::new();
    let mut response = execute(client, request, credential, &mut send)
        .await
        .map_err(|error| {
            RedactedDetail::internal(
                ProviderFailureKind::Transport,
                if error.proof() == DispatchProof::PossiblySent {
                    "the negative probe transport failed after dispatch"
                } else {
                    "the negative probe was rejected before dispatch"
                },
            )
        })?;
    let status = response.status().as_u16();
    if response.status().is_success() {
        return Err(RedactedDetail::internal(
            ProviderFailureKind::InvalidRequest,
            "a negative qualification request unexpectedly succeeded",
        ));
    }
    let headers = response.headers().clone();
    scan_headers(scanner, &headers);
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        RedactedDetail::internal(
            ProviderFailureKind::Transport,
            "the negative response body could not be read",
        )
    })? {
        scan_artifact(scanner, &chunk);
        let remaining = BODY_LIMIT.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = BoundedBody::new(bytes, truncated);
    let shape = error_body_shape(&body);
    let failure = DeepSeekAdapter.classify_http(status, &HeaderView::new(&headers), &body);
    Ok(ClassifiedNegative {
        kind: failure.kind(),
        shape,
    })
}

fn error_body_shape(body: &BoundedBody) -> ErrorBodyShape {
    if body.as_bytes().is_empty() {
        return ErrorBodyShape::StatusOnly;
    }
    let Some(error) = body.as_json().and_then(|value| value.get("error").cloned()) else {
        return ErrorBodyShape::OtherRedacted;
    };
    if error.get("message").is_some()
        && (error.get("type").is_some() || error.get("code").is_some())
    {
        ErrorBodyShape::OpenAiEnvelope
    } else {
        ErrorBodyShape::OtherRedacted
    }
}

/// Local validation failures which never contain provider material.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum QualificationError {
    /// The protected environment supplied no credential bytes.
    #[error("the protected DeepSeek credential is empty")]
    EmptyCredential,
    /// The workflow run identity was absent or malformed.
    #[error("the qualification run identity is invalid")]
    InvalidRunIdentity,
}

/// Fail-closed protected-run errors. None carry provider response bytes.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum QualificationRunError {
    /// One or more unconditional programs still lack a production request seam.
    #[error("required qualification programs are not implemented: {probes:?}")]
    IncompletePrograms {
        /// Exact missing probe identities.
        probes: Vec<ProbeId>,
    },
    /// The caller supplied a staged entry for another pair or policy shape.
    #[error("the staged candidate does not match deepseek/deepseek-v4-flash")]
    CandidateMismatch,
    /// The caller did not supply the exact reviewed tokenizer-derived messages.
    #[error("the qualification corpora do not match the pinned tokenizer, template, and messages")]
    CorpusMismatch,
    /// The deterministic worst-case token spend exceeds operator authority.
    #[error(
        "qualification requires at most {required_micro_usd} micro-USD, above the approved {approved_micro_usd} micro-USD"
    )]
    BudgetInsufficient {
        /// Pessimistic cost bound computed before credential or network access.
        required_micro_usd: u64,
        /// Operator-approved bound.
        approved_micro_usd: u64,
    },
    /// The bounded HTTP client could not be assembled.
    #[error("the bounded qualification HTTP client could not be configured")]
    ClientConfiguration,
    /// The exact target or program inventory is invalid.
    #[error(transparent)]
    Readiness(#[from] ReadinessError),
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aex_model_catalog::canonical::{NormalizedUsage, UsageCompleteness};
    use aex_model_catalog::receipt::ProbeId;

    use super::{fault_evidence, incomplete_matrix_usage};

    #[test]
    fn local_fault_shapes_are_not_live_evidence() {
        for probe in ProbeId::ALL {
            assert!(fault_evidence(probe).is_none());
        }
    }

    #[test]
    fn incomplete_matrix_usage_preserves_every_observed_numeric_total() {
        let observed = NormalizedUsage {
            input_tokens: 11,
            cache_read_input_tokens: 12,
            cache_write_input_tokens: 13,
            output_tokens: 14,
            reasoning_tokens: 5,
            tool_use_prompt_tokens: 16,
            provider_total_tokens: Some(66),
            completeness: UsageCompleteness::Exact,
        };
        let summarized = incomplete_matrix_usage(&Mutex::new(observed));
        assert_eq!(summarized.input_tokens, 11);
        assert_eq!(summarized.cache_read_input_tokens, 12);
        assert_eq!(summarized.cache_write_input_tokens, 13);
        assert_eq!(summarized.output_tokens, 14);
        assert_eq!(summarized.reasoning_tokens, 5);
        assert_eq!(summarized.tool_use_prompt_tokens, 16);
        assert_eq!(summarized.provider_total_tokens, None);
        assert_eq!(summarized.completeness, UsageCompleteness::Absent);
    }
}
