//! Bounded SSRF-safe URL ingestion for latest-only workspace files.
//!
//! The public file has no version. The private pointer revision is used only as
//! a stale-completion fence: a slow fetch can store immutable orphan bytes, but
//! it can never replace a newer current intent.

use std::time::{Duration, Instant};

use aex_brain_managed_web::egress::{
    DnsResolver, EgressPolicy, EgressRejection, SystemDnsResolver, resolve_and_screen, validate,
};
use aex_content_domain::identity::RegistryKind;
use aex_wire::ids::{OrganizationId, ResourceName, WorkspaceId};
use aex_wire::models;
use aex_workspace_domain::registry::{RegistryPointer, RegistryState};
use bytes::BytesMut;
use futures::StreamExt as _;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use serde::Deserialize;

/// Largest URL-sourced body admitted by the launch worker.
pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
/// Complete wall-clock bound including redirects and body progress.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
const TTFB_TIMEOUT: Duration = Duration::from_secs(15);
const PROGRESS_TIMEOUT: Duration = Duration::from_secs(10);

/// Private pending value document written by `session-api`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingUrlFile {
    /// Exact authenticated organization used by the content descriptor.
    pub organization_id: OrganizationId,
    /// URL source.
    pub source: UrlSource,
    /// Customer-declared media type retained on the file value.
    pub media_type: String,
    /// Requested file mode.
    pub mode: models::RegisteredFileMode,
}

/// Exact private URL source.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UrlSource {
    /// Closed source discriminator.
    #[serde(rename = "type")]
    pub kind: String,
    /// Exact initial URL.
    pub url: String,
}

/// Verified bytes returned by the bounded fetcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedFile {
    /// Exact response bytes, without transparent content decoding.
    pub bytes: Vec<u8>,
    /// Observed origin media type when supplied.
    pub observed_media_type: Option<String>,
}

/// Stable failure family safe to publish as `failureCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FetchFailure {
    /// URL syntax, scheme, port, credentials, DNS or address policy refusal.
    #[error("source URL is not allowed")]
    SourceRejected,
    /// Redirect chain was invalid or exceeded its bound.
    #[error("source redirect was refused")]
    RedirectRejected,
    /// Origin did not return a successful status.
    #[error("source returned a non-success status")]
    SourceStatus,
    /// Body exceeded the exact launch bound.
    #[error("source body exceeds the file limit")]
    SourceTooLarge,
    /// Connect, TTFB, progress or total deadline elapsed.
    #[error("source fetch timed out")]
    SourceTimeout,
    /// Other bounded transport failure.
    #[error("source fetch failed")]
    SourceUnavailable,
}

impl FetchFailure {
    /// Stable bounded public failure code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SourceRejected => "source_rejected",
            Self::RedirectRejected => "redirect_rejected",
            Self::SourceStatus => "source_status",
            Self::SourceTooLarge => "source_too_large",
            Self::SourceTimeout => "source_timeout",
            Self::SourceUnavailable => "source_unavailable",
        }
    }
}

/// Decodes only a current pending URL file pointer.
///
/// # Errors
///
/// Returns a stable failure when the stream row is another kind/state/source or
/// the private document is malformed.
pub fn pending_url(pointer: &RegistryPointer) -> Result<PendingUrlFile, PendingError> {
    if pointer.row.kind != RegistryKind::File || pointer.row.state != RegistryState::Pending {
        return Err(PendingError::NotPendingUrl);
    }
    let pending: PendingUrlFile =
        serde_json::from_str(pointer.value_doc.as_str()).map_err(|_| PendingError::Malformed)?;
    if pending.source.kind != "url" {
        return Err(PendingError::NotPendingUrl);
    }
    if pending.media_type.is_empty() || pending.media_type.len() > 255 {
        return Err(PendingError::Malformed);
    }
    Ok(pending)
}

/// Why a pointer is not an ingestible URL intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PendingError {
    /// A normal stream row this worker ignores.
    #[error("pointer is not a pending URL file")]
    NotPendingUrl,
    /// A row claiming to be pending URL state has a corrupt private document.
    #[error("pending URL file document is malformed")]
    Malformed,
}

/// Fetches arbitrary bytes through an HTTPS-only, DNS-screened, pinned-address
/// client. Redirects are re-resolved and screened before every connection.
pub async fn fetch_url(url: &str) -> Result<FetchedFile, FetchFailure> {
    tokio::time::timeout(
        FETCH_TIMEOUT,
        fetch_with_resolver(url, &SystemDnsResolver, MAX_FILE_BYTES),
    )
    .await
    .map_err(|_| FetchFailure::SourceTimeout)?
}

/// Resolver-injected form used by security tests.
pub async fn fetch_with_resolver(
    url: &str,
    resolver: &dyn DnsResolver,
    max_bytes: usize,
) -> Result<FetchedFile, FetchFailure> {
    if max_bytes == 0 || max_bytes > MAX_FILE_BYTES {
        return Err(FetchFailure::SourceTooLarge);
    }
    let policy = EgressPolicy::managed_web();
    let mut next = url.to_owned();
    let mut redirects = 0_u8;
    loop {
        let parsed = validate(&policy, &next).map_err(classify_egress)?;
        let target =
            tokio::time::timeout(Duration::from_secs(5), resolve_and_screen(parsed, resolver))
                .await
                .map_err(|_| FetchFailure::SourceTimeout)?
                .map_err(classify_egress)?;
        let pinned = target
            .addrs
            .iter()
            .map(|address| std::net::SocketAddr::new(*address, 443))
            .collect::<Vec<_>>();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .no_gzip()
            .no_brotli()
            .resolve_to_addrs(&target.host, &pinned)
            .build()
            .map_err(|_| FetchFailure::SourceUnavailable)?;
        let response = tokio::time::timeout(
            TTFB_TIMEOUT,
            client
                .get(target.url.clone())
                .header(ACCEPT, "*/*")
                .header(ACCEPT_ENCODING, "identity")
                .send(),
        )
        .await
        .map_err(|_| FetchFailure::SourceTimeout)?
        .map_err(|_| FetchFailure::SourceUnavailable)?;
        if response.status().is_redirection() {
            if redirects >= policy.max_redirects {
                return Err(FetchFailure::RedirectRejected);
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(FetchFailure::RedirectRejected)?;
            next = target
                .url
                .join(location)
                .map_err(|_| FetchFailure::RedirectRejected)?
                .to_string();
            redirects = redirects.saturating_add(1);
            continue;
        }
        if !response.status().is_success() {
            return Err(FetchFailure::SourceStatus);
        }
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > max_bytes)
        {
            return Err(FetchFailure::SourceTooLarge);
        }
        let observed_media_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or(value).trim().to_owned());
        let mut body = BytesMut::new();
        let mut stream = response.bytes_stream();
        let started = Instant::now();
        while let Some(chunk) = tokio::time::timeout(PROGRESS_TIMEOUT, stream.next())
            .await
            .map_err(|_| FetchFailure::SourceTimeout)?
        {
            let chunk = chunk.map_err(|_| FetchFailure::SourceUnavailable)?;
            if body.len().saturating_add(chunk.len()) > max_bytes {
                return Err(FetchFailure::SourceTooLarge);
            }
            body.extend_from_slice(&chunk);
            if started.elapsed() > FETCH_TIMEOUT {
                return Err(FetchFailure::SourceTimeout);
            }
        }
        return Ok(FetchedFile {
            bytes: body.to_vec(),
            observed_media_type,
        });
    }
}

fn classify_egress(error: EgressRejection) -> FetchFailure {
    match error {
        EgressRejection::RedirectLoop
        | EgressRejection::RedirectLimit
        | EgressRejection::RedirectMissingLocation => FetchFailure::RedirectRejected,
        _ => FetchFailure::SourceRejected,
    }
}

/// Builds the public ready value document for exact fetched bytes.
pub fn ready_document(
    pending: &PendingUrlFile,
    digest: aex_wire::ids::ContentHash,
    size_bytes: u64,
) -> models::RegisteredFileRead {
    models::RegisteredFileRead {
        content: models::ContentRef {
            sha256: digest,
            size_bytes: aex_wire::types::DecimalU128::new(u128::from(size_bytes)),
        },
        media_type: pending.media_type.clone(),
        mode: pending.mode,
    }
}

/// Minimal pointer locator decoded from a `KEYS_ONLY` stream record before the
/// authoritative point read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamLocator {
    /// Workspace owner.
    pub workspace_id: WorkspaceId,
    /// Current logical name.
    pub name: ResourceName,
}

#[derive(Debug, Deserialize)]
struct StreamKey {
    pk: String,
    sk: String,
}

/// Returns a file-pointer candidate from stream keys. State and source type are
/// deliberately absent from the event and are reloaded from authority before
/// any network effect.
#[must_use]
pub fn stream_candidate(item: serde_dynamo::Item) -> Option<StreamLocator> {
    let key: StreamKey = serde_dynamo::from_item(item).ok()?;
    let partition = key.pk.strip_prefix("REG#")?;
    let (workspace, kind) = partition.rsplit_once('#')?;
    if kind != "file" {
        return None;
    }
    let name = key.sk.strip_prefix("NAME#")?;
    Some(StreamLocator {
        workspace_id: WorkspaceId::parse(workspace).ok()?,
        name: ResourceName::parse(name).ok()?,
    })
}
