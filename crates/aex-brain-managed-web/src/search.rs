//! Closed BYOK web-search authority and provider normalizers.

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use futures::StreamExt as _;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, HeaderName};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};

use crate::egress::{DnsResolver, EgressPolicy, EgressRejection, resolve_and_screen, validate};

/// The only workspace secret name that grants managed search authority.
pub const WEB_SEARCH_SECRET_NAME: &str = "aex_web_search";
const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const SERPER_ENDPOINT: &str = "https://google.serper.dev/search";
const RESPONSE_LIMIT: usize = 500_000;

/// Closed provider vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProviderId {
    /// Brave Search API.
    Brave,
    /// Serper Google Search API.
    Serper,
}

/// Parsed workspace authority. The key is zeroized on drop and never exposed
/// through `Debug`.
pub struct WebSearchCredential {
    provider: WebSearchProviderId,
    api_key: SecretString,
}

impl WebSearchCredential {
    /// Parses the exact secret payload `{provider, apiKey}`.
    ///
    /// # Errors
    ///
    /// Rejects malformed JSON, unknown fields/providers, and an empty key.
    pub fn parse(bytes: &[u8]) -> Result<Self, SearchRejection> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct RawCredential {
            provider: WebSearchProviderId,
            api_key: SecretString,
        }

        let raw: RawCredential =
            serde_json::from_slice(bytes).map_err(|_| SearchRejection::CredentialMalformed)?;
        if raw.api_key.expose_secret().is_empty() {
            return Err(SearchRejection::CredentialMalformed);
        }
        Ok(Self {
            provider: raw.provider,
            api_key: raw.api_key,
        })
    }

    /// Provider selected by the authority.
    #[must_use]
    pub const fn provider(&self) -> WebSearchProviderId {
        self.provider
    }
}

impl fmt::Debug for WebSearchCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebSearchCredential")
            .field("provider", &self.provider)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

/// Provider freshness vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchFreshness {
    /// Last day.
    Day,
    /// Last week.
    Week,
    /// Last month.
    Month,
    /// Last year.
    Year,
}

/// One validated managed-search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchRequest<'a> {
    /// Search query, 1..=2000 bytes.
    pub query: &'a str,
    /// Result ceiling, 1..=20.
    pub count: u8,
    /// Optional two-uppercase-letter country.
    pub country: Option<&'a str>,
    /// Optional provider-independent freshness.
    pub freshness: Option<SearchFreshness>,
}

/// One provider-independent search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// One-based rank assigned from the provider order after invalid entries are dropped.
    pub rank: u8,
    /// Result title.
    pub title: String,
    /// Result URL.
    pub url: String,
    /// Optional snippet.
    pub snippet: Option<String>,
    /// Optional RFC 3339 provider timestamp.
    pub published_at: Option<String>,
}

/// Stable search result shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchResult {
    /// Authority that produced the response.
    pub provider: WebSearchProviderId,
    /// Original query.
    pub query: String,
    /// Results in provider order.
    pub results: Vec<SearchResult>,
    /// Whether invalid or excess provider entries were omitted.
    pub truncated: bool,
}

/// Dispatches one request to the credential's pinned endpoint through the same
/// complete-set DNS screening used by fetch.
///
/// # Errors
///
/// Returns a redacted typed failure; provider response bodies never enter errors.
pub async fn search(
    request: WebSearchRequest<'_>,
    credential: &WebSearchCredential,
    resolver: &dyn DnsResolver,
) -> Result<WebSearchResult, SearchRejection> {
    validate_request(&request)?;
    let endpoint = match credential.provider {
        WebSearchProviderId::Brave => BRAVE_ENDPOINT,
        WebSearchProviderId::Serper => SERPER_ENDPOINT,
    };
    let target =
        resolve_and_screen(validate(&EgressPolicy::managed_web(), endpoint)?, resolver).await?;
    let pinned = target
        .addrs
        .iter()
        .map(|address| SocketAddr::new(*address, 443))
        .collect::<Vec<_>>();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .resolve_to_addrs(&target.host, &pinned)
        .build()
        .map_err(|_| SearchRejection::Transport)?;

    let response = match credential.provider {
        WebSearchProviderId::Brave => {
            let mut query = vec![
                ("q", request.query.to_owned()),
                ("count", request.count.to_string()),
            ];
            if let Some(country) = request.country {
                query.push(("country", country.to_owned()));
            }
            if let Some(freshness) = request.freshness {
                query.push(("freshness", freshness_name(freshness).to_owned()));
            }
            client
                .get(target.url)
                .header(ACCEPT, "application/json")
                .header(
                    HeaderName::from_static("x-subscription-token"),
                    credential.api_key.expose_secret(),
                )
                .query(&query)
                .send()
                .await
        }
        WebSearchProviderId::Serper => {
            let mut body = serde_json::json!({"q": request.query, "num": request.count});
            let Some(object) = body.as_object_mut() else {
                return Err(SearchRejection::InvalidRequest);
            };
            if let Some(country) = request.country {
                object.insert("gl".into(), country.to_ascii_lowercase().into());
            }
            if let Some(freshness) = request.freshness {
                object.insert("tbs".into(), serper_freshness(freshness).into());
            }
            client
                .post(target.url)
                .header(ACCEPT, "application/json")
                .header(CONTENT_TYPE, "application/json")
                .header(
                    HeaderName::from_static("x-api-key"),
                    credential.api_key.expose_secret(),
                )
                .json(&body)
                .send()
                .await
        }
    }
    .map_err(|_| SearchRejection::Transport)?;
    if !response.status().is_success() {
        return Err(SearchRejection::ProviderStatus {
            status: response.status().as_u16(),
        });
    }
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > RESPONSE_LIMIT)
    {
        return Err(SearchRejection::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| SearchRejection::Transport)?;
        bytes.extend_from_slice(&chunk);
        if bytes.len() > RESPONSE_LIMIT {
            return Err(SearchRejection::ResponseTooLarge);
        }
    }
    let value = serde_json::from_slice(&bytes).map_err(|_| SearchRejection::ResponseMalformed)?;
    match credential.provider {
        WebSearchProviderId::Brave => normalize_brave(&value, request.query, request.count),
        WebSearchProviderId::Serper => normalize_serper(&value, request.query, request.count),
    }
}

fn validate_request(request: &WebSearchRequest<'_>) -> Result<(), SearchRejection> {
    if request.query.is_empty() || request.query.len() > 2_000 {
        return Err(SearchRejection::InvalidRequest);
    }
    if !(1..=20).contains(&request.count) {
        return Err(SearchRejection::InvalidRequest);
    }
    if request.country.is_some_and(|country| {
        country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase())
    }) {
        return Err(SearchRejection::InvalidRequest);
    }
    Ok(())
}

/// Normalizes a recorded Brave response fixture.
///
/// # Errors
///
/// Rejects a non-object response or an invalid caller count.
pub fn normalize_brave(
    response: &serde_json::Value,
    query: &str,
    count: u8,
) -> Result<WebSearchResult, SearchRejection> {
    let response = response
        .as_object()
        .ok_or(SearchRejection::ResponseMalformed)?;
    let items = response
        .get("web")
        .and_then(serde_json::Value::as_object)
        .and_then(|web| web.get("results"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    normalize_items(
        WebSearchProviderId::Brave,
        query,
        count,
        items,
        "url",
        "description",
        "age",
    )
}

/// Normalizes a recorded Serper response fixture.
///
/// # Errors
///
/// Rejects a non-object response or an invalid caller count.
pub fn normalize_serper(
    response: &serde_json::Value,
    query: &str,
    count: u8,
) -> Result<WebSearchResult, SearchRejection> {
    let response = response
        .as_object()
        .ok_or(SearchRejection::ResponseMalformed)?;
    let items = response
        .get("organic")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    normalize_items(
        WebSearchProviderId::Serper,
        query,
        count,
        items,
        "link",
        "snippet",
        "date",
    )
}

fn normalize_items(
    provider: WebSearchProviderId,
    query: &str,
    count: u8,
    items: &[serde_json::Value],
    url_field: &str,
    snippet_field: &str,
    date_field: &str,
) -> Result<WebSearchResult, SearchRejection> {
    if !(1..=20).contains(&count) {
        return Err(SearchRejection::InvalidRequest);
    }
    let mut results = Vec::new();
    let mut truncated = false;
    for item in items {
        let Some(item) = item.as_object() else {
            truncated = true;
            continue;
        };
        let title = item.get("title").and_then(serde_json::Value::as_str);
        let url = item.get(url_field).and_then(serde_json::Value::as_str);
        let Some((title, url)) = title.zip(url).filter(|(title, url)| {
            !title.is_empty() && title.len() <= 512 && !url.is_empty() && url.len() <= 4_096
        }) else {
            truncated = true;
            continue;
        };
        if results.len() == usize::from(count) {
            truncated = true;
            continue;
        }
        let snippet = item
            .get(snippet_field)
            .and_then(serde_json::Value::as_str)
            .filter(|snippet| snippet.len() <= 2_048)
            .map(str::to_owned);
        let published_at = item
            .get(date_field)
            .and_then(serde_json::Value::as_str)
            .filter(|date| date.len() <= 64)
            .map(str::to_owned);
        results.push(SearchResult {
            rank: u8::try_from(results.len() + 1).expect("count is capped at twenty"),
            title: title.to_owned(),
            url: url.to_owned(),
            snippet,
            published_at,
        });
    }
    Ok(WebSearchResult {
        provider,
        query: query.to_owned(),
        results,
        truncated,
    })
}

const fn freshness_name(freshness: SearchFreshness) -> &'static str {
    match freshness {
        SearchFreshness::Day => "day",
        SearchFreshness::Week => "week",
        SearchFreshness::Month => "month",
        SearchFreshness::Year => "year",
    }
}

const fn serper_freshness(freshness: SearchFreshness) -> &'static str {
    match freshness {
        SearchFreshness::Day => "qdr:d",
        SearchFreshness::Week => "qdr:w",
        SearchFreshness::Month => "qdr:m",
        SearchFreshness::Year => "qdr:y",
    }
}

/// Search authority or provider failure. No variant carries credential or body text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchRejection {
    /// Workspace secret did not match its closed schema.
    #[error("managed search credential is malformed")]
    CredentialMalformed,
    /// Public request was outside its schema bounds.
    #[error("managed search request is invalid")]
    InvalidRequest,
    /// Shared egress policy refused the provider endpoint.
    #[error(transparent)]
    Egress(#[from] EgressRejection),
    /// Network operation failed without exposing response detail.
    #[error("managed search transport failed")]
    Transport,
    /// Provider returned a non-success status.
    #[error("managed search provider returned HTTP {status}")]
    ProviderStatus {
        /// Observed status only; response body is discarded.
        status: u16,
    },
    /// Response exceeded its fixed ceiling.
    #[error("managed search response exceeds 500000 bytes")]
    ResponseTooLarge,
    /// Provider response could not be normalized.
    #[error("managed search provider response is malformed")]
    ResponseMalformed,
}
