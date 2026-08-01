//! Bounded, pinned-address fetch and deterministic response formatting.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use aex_wire::ids::ContentHash;
use encoding_rs::Encoding;
use futures::StreamExt as _;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::egress::ValidatedTarget;
use crate::egress::{DnsResolver, EgressPolicy, EgressRejection, resolve_and_screen, validate};
use crate::serializer::{html_to_markdown, html_to_text};

const IO_SAFETY_BYTES: usize = 500_000;
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const TTFB_TIMEOUT: Duration = Duration::from_secs(10);
const PROGRESS_WINDOW: Duration = Duration::from_secs(5);
const MIN_PROGRESS_BYTES: usize = 1_024;

/// Requested model-facing representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchFormat {
    /// Deterministic Markdown for HTML; decoded bytes for textual non-HTML.
    Markdown,
    /// Markup-free collapsed text.
    Text,
    /// Decoded body without markup transformation.
    Raw,
}

/// One bounded fetch request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest<'a> {
    /// HTTPS URL to retrieve.
    pub url: &'a str,
    /// Requested representation.
    pub format: FetchFormat,
    /// Caller ceiling, additionally capped by the session I/O policy.
    pub max_bytes: usize,
}

/// The allowed textual media vocabulary.
#[must_use]
pub fn media_type_allowed(media_type: &str) -> bool {
    matches!(
        media_type.trim().to_ascii_lowercase().as_str(),
        "text/html"
            | "application/xhtml+xml"
            | "text/plain"
            | "text/markdown"
            | "text/csv"
            | "text/xml"
            | "application/xml"
            | "application/json"
            | "application/ld+json"
    )
}

/// Fetches one document through a fresh client pinned to the screened address
/// set for each hop.
///
/// # Errors
///
/// Refuses policy, transport, redirect, timeout, media, and size failures.
pub async fn fetch(
    request: FetchRequest<'_>,
    resolver: &dyn DnsResolver,
) -> Result<FetchDocument, FetchRejection> {
    if !(1_024..=IO_SAFETY_BYTES).contains(&request.max_bytes) {
        return Err(FetchRejection::InvalidMaxBytes {
            value: request.max_bytes,
        });
    }
    tokio::time::timeout(TOTAL_TIMEOUT, fetch_inner(request, resolver))
        .await
        .map_err(|_| FetchRejection::TotalTimeout)?
}

async fn fetch_inner(
    request: FetchRequest<'_>,
    resolver: &dyn DnsResolver,
) -> Result<FetchDocument, FetchRejection> {
    let policy = EgressPolicy::managed_web();
    let mut next_url = request.url.to_owned();
    let mut redirects = Vec::new();
    let mut visited = BTreeSet::new();

    loop {
        let parsed = validate(&policy, &next_url)?;
        let normalized = parsed.url.as_str().to_owned();
        if !visited.insert(normalized.clone()) {
            return Err(EgressRejection::RedirectLoop.into());
        }
        let target =
            tokio::time::timeout(Duration::from_secs(3), resolve_and_screen(parsed, resolver))
                .await
                .map_err(|_| EgressRejection::DnsFailure)??;
        let response = send(&target).await?;

        if response.status().is_redirection() {
            if redirects.len() == usize::from(policy.max_redirects) {
                return Err(EgressRejection::RedirectLimit.into());
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(EgressRejection::RedirectMissingLocation)?;
            let redirected = target
                .url
                .join(location)
                .map_err(|_| EgressRejection::RedirectMissingLocation)?;
            redirected.as_str().clone_into(&mut next_url);
            redirects.push(next_url.clone());
            continue;
        }

        let downloaded = read_response(response, request.max_bytes).await?;

        let (media_type, charset) = parse_content_type(&downloaded.content_type);
        let content = format_document(
            &downloaded.body,
            &downloaded.content_type,
            request.format,
            &target.url,
        )?;
        return Ok(FetchDocument {
            url: request.url.to_owned(),
            final_url: target.url.into(),
            status: downloaded.status,
            media_type: media_type.to_owned(),
            charset: charset.map(str::to_owned),
            format: request.format,
            bytes: u32::try_from(downloaded.body.len()).expect("managed-web ceiling fits u32"),
            truncated: false,
            redirects,
            content,
            locator: ContentHash::of(&downloaded.body),
        });
    }
}

async fn send(target: &ValidatedTarget) -> Result<reqwest::Response, FetchRejection> {
    let pinned = target
        .addrs
        .iter()
        .map(|address| SocketAddr::new(*address, 443))
        .collect::<Vec<_>>();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(TOTAL_TIMEOUT)
        .gzip(true)
        .brotli(true)
        .resolve_to_addrs(&target.host, &pinned)
        .build()
        .map_err(|_| FetchRejection::ClientBuild)?;
    tokio::time::timeout(
        TTFB_TIMEOUT,
        client
            .get(target.url.clone())
            .header(ACCEPT, "text/html, application/xhtml+xml, text/plain, text/markdown, text/csv, text/xml, application/xml, application/json, application/ld+json")
            .header(ACCEPT_ENCODING, "gzip, br")
            .send(),
    )
    .await
    .map_err(|_| FetchRejection::TimeToFirstByteTimeout)?
    .map_err(|_| FetchRejection::Transport)
}

struct DownloadedResponse {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

async fn read_response(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<DownloadedResponse, FetchRejection> {
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or(FetchRejection::MissingContentType)?
        .to_owned();
    let declared_length = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared_length.is_some_and(|length| length > max_bytes) {
        return Err(FetchRejection::BodyTooLarge { limit: max_bytes });
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    let mut window = (Instant::now(), 0usize);
    while let Some(chunk) = tokio::time::timeout(PROGRESS_WINDOW, stream.next())
        .await
        .map_err(|_| FetchRejection::MinimumProgress)?
    {
        let chunk = chunk.map_err(|_| FetchRejection::Transport)?;
        body.extend_from_slice(&chunk);
        window.1 = window.1.saturating_add(chunk.len());
        enforce_stream_bounds(&body, declared_length, max_bytes, &mut window)?;
    }
    Ok(DownloadedResponse {
        status,
        content_type,
        body,
    })
}

fn enforce_stream_bounds(
    body: &[u8],
    declared_length: Option<usize>,
    max_bytes: usize,
    window: &mut (Instant, usize),
) -> Result<(), FetchRejection> {
    if body.len() > max_bytes {
        return Err(FetchRejection::BodyTooLarge { limit: max_bytes });
    }
    if declared_length
        .is_some_and(|encoded| encoded > 0 && body.len() > encoded.saturating_mul(100))
    {
        return Err(FetchRejection::CompressionRatio);
    }
    if window.0.elapsed() >= PROGRESS_WINDOW {
        if window.1 < MIN_PROGRESS_BYTES {
            return Err(FetchRejection::MinimumProgress);
        }
        *window = (Instant::now(), 0);
    }
    Ok(())
}

/// Decodes a bounded response body and applies the requested stable format.
///
/// # Errors
///
/// Refuses media outside the closed allowlist and unknown declared charsets.
pub fn format_document(
    body: &[u8],
    content_type: &str,
    format: FetchFormat,
    base: &Url,
) -> Result<String, FetchRejection> {
    let (media_type, charset) = parse_content_type(content_type);
    if !media_type_allowed(media_type) {
        return Err(FetchRejection::MediaTypeNotAllowed {
            media_type: media_type.to_owned(),
        });
    }
    let encoding = charset
        .map(|label| {
            Encoding::for_label(label.as_bytes()).ok_or_else(|| {
                FetchRejection::CharsetNotSupported {
                    charset: label.to_owned(),
                }
            })
        })
        .transpose()?
        .unwrap_or(encoding_rs::UTF_8);
    let (decoded, _, _) = encoding.decode(body);
    let is_html = matches!(media_type, "text/html" | "application/xhtml+xml");
    Ok(match (format, is_html) {
        (FetchFormat::Raw, _) | (FetchFormat::Markdown, false) => decoded.into_owned(),
        (FetchFormat::Text, true) => html_to_text(&decoded),
        (FetchFormat::Text, false) => decoded.split_whitespace().collect::<Vec<_>>().join(" "),
        (FetchFormat::Markdown, true) => html_to_markdown(&decoded, base),
    })
}

fn parse_content_type(content_type: &str) -> (&str, Option<&str>) {
    let mut parts = content_type.split(';');
    let media_type = parts.next().unwrap_or_default().trim();
    let charset = parts.find_map(|part| {
        let part = part.trim();
        let (name, value) = part.split_once('=')?;
        name.eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"'))
    });
    (media_type, charset)
}

/// A successful bounded fetch document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchDocument {
    /// Original requested URL.
    pub url: String,
    /// Last URL after redirects.
    pub final_url: String,
    /// HTTP response status.
    pub status: u16,
    /// Normalized media type.
    pub media_type: String,
    /// Declared response charset, if present.
    pub charset: Option<String>,
    /// Applied representation.
    pub format: FetchFormat,
    /// Decoded source bytes before formatting.
    pub bytes: u32,
    /// Always false; oversize input is refused.
    pub truncated: bool,
    /// Revalidated redirect targets.
    pub redirects: Vec<String>,
    /// Model-facing content.
    pub content: String,
    /// SHA-256 retrieval identity.
    pub locator: ContentHash,
}

/// A fetched response could not be represented safely.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchRejection {
    /// Shared outbound policy rejected the target.
    #[error(transparent)]
    Egress(#[from] EgressRejection),
    /// Requested bound was outside the public schema.
    #[error("managed web maxBytes {value} is outside 1024..=500000")]
    InvalidMaxBytes {
        /// Refused bound.
        value: usize,
    },
    /// Media type was outside the textual allowlist.
    #[error("managed web media type `{media_type}` is not allowed")]
    MediaTypeNotAllowed {
        /// Refused normalized media type.
        media_type: String,
    },
    /// A declared charset had no deterministic decoder.
    #[error("managed web charset `{charset}` is not supported")]
    CharsetNotSupported {
        /// Refused charset label.
        charset: String,
    },
    /// A response exceeded the effective byte ceiling.
    #[error("managed web body exceeds {limit} bytes")]
    BodyTooLarge {
        /// Effective byte ceiling.
        limit: usize,
    },
    /// Client construction failed before dispatch.
    #[error("managed web client construction failed")]
    ClientBuild,
    /// The server did not return a Content-Type.
    #[error("managed web response omitted Content-Type")]
    MissingContentType,
    /// No response headers arrived in ten seconds.
    #[error("managed web time-to-first-byte deadline exceeded")]
    TimeToFirstByteTimeout,
    /// The total request wall exceeded thirty seconds.
    #[error("managed web total deadline exceeded")]
    TotalTimeout,
    /// The response stream failed to sustain 1 KiB per five-second window.
    #[error("managed web response made insufficient progress")]
    MinimumProgress,
    /// Decompression exceeded the maximum admitted expansion ratio.
    #[error("managed web compression ratio exceeded 100:1")]
    CompressionRatio,
    /// HTTP or response streaming failed.
    #[error("managed web transport failed")]
    Transport,
}
