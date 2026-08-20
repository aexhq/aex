//! Aex-managed web server Tools.
//!
//! These execute in the trusted host service. They never run in the customer-controlled Hand and
//! never place the operator's search credential in a session, prompt, journal, or MicroVM.

use std::time::Duration;

use brain_protocol::session::ExternalToolCallRequest;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::outbound::Outbound;
use crate::{Error, Result};

pub const SEARCH_TOOL_NAME: &str = "web_search";
pub const SEARCH_CAPABILITY: &str = "aex.web.search.v1";
pub const FETCH_TOOL_NAME: &str = "web_fetch";
pub const FETCH_CAPABILITY: &str = "aex.web.fetch.v1";

const SEARCH_ENDPOINT: &str = "https://google.serper.dev/search";
const MAX_RESULT_BYTES: usize = 96 * 1024;
const SEARCH_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const FETCH_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const FETCH_MAX_CHARS: usize = 100_000;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    query: String,
    #[serde(default = "default_results")]
    num: usize,
    country: Option<String>,
    language: Option<String>,
}

fn default_results() -> usize {
    5
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchInput {
    url: String,
    max_chars: Option<usize>,
}

#[derive(Clone)]
pub struct WebRuntime {
    outbound: Outbound,
    search_key: Option<String>,
    timeout: Duration,
}

impl std::fmt::Debug for WebRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebRuntime")
            .field("outbound", &self.outbound)
            .field(
                "search_key",
                &self.search_key.as_ref().map(|_| "<redacted>"),
            )
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl WebRuntime {
    pub fn hosted(search_key: Option<String>) -> Self {
        Self {
            outbound: Outbound::guarded(),
            search_key,
            timeout: Duration::from_secs(30),
        }
    }

    /// Execute only the pinned Aex name/capability pairs. Capability metadata is added by Brain's
    /// trusted executor adapter and cannot be supplied in model arguments.
    pub async fn execute(&self, request: ExternalToolCallRequest) -> Result<Value> {
        let capability = request
            .context
            .get("brain.capability")
            .map(String::as_str)
            .ok_or_else(|| Error::Invalid("the sealed server capability is missing".into()))?;
        let is_search = match (request.name.as_str(), capability) {
            (SEARCH_TOOL_NAME, SEARCH_CAPABILITY) => true,
            (FETCH_TOOL_NAME, FETCH_CAPABILITY) => false,
            _ => {
                return Err(Error::Invalid(format!(
                    "unknown hosted server Tool {:?} for capability {capability:?}",
                    request.name
                )));
            }
        };
        let operation = async {
            if is_search {
                self.search(&request.input).await
            } else {
                self.fetch(&request.input).await
            }
        };
        match tokio::time::timeout(self.timeout, operation).await {
            Err(_) => Ok(tool_failure(
                "deadline_exceeded",
                format!(
                    "{} exceeded the {} ms host deadline",
                    request.name,
                    self.timeout.as_millis()
                ),
            )),
            Ok(Err(error)) => Ok(tool_failure("failed", safe_error(&error))),
            Ok(Ok(content)) => {
                let result: Value = serde_json::from_str(&content).map_err(|error| {
                    Error::Internal(format!("managed web Tool result encoding: {error}"))
                })?;
                Ok(json!({
                    "outcome": "completed",
                    "content": content,
                    "is_error": false,
                    "disposition": "continue",
                    "result": result
                }))
            }
        }
    }

    async fn search(&self, input: &Value) -> Result<String> {
        let input: SearchInput = serde_json::from_value(input.clone())
            .map_err(|error| Error::Invalid(format!("web_search input: {error}")))?;
        let query = input.query.trim();
        if query.is_empty() || query.chars().count() > 500 {
            return Err(Error::Invalid(
                "web_search.query must contain 1 through 500 characters".into(),
            ));
        }
        if !(1..=10).contains(&input.num) {
            return Err(Error::Invalid(
                "web_search.num must be between 1 and 10".into(),
            ));
        }
        for (name, value, maximum) in [
            ("country", input.country.as_deref(), 8usize),
            ("language", input.language.as_deref(), 16usize),
        ] {
            if let Some(value) = value
                && (value.len() < 2 || value.len() > maximum)
            {
                return Err(Error::Invalid(format!(
                    "web_search.{name} must contain 2 through {maximum} bytes"
                )));
            }
        }
        let key = self.search_key.as_ref().ok_or_else(|| {
            Error::Invalid("managed web search is not configured on this Aex plane".into())
        })?;
        let endpoint = self.outbound.check_url(SEARCH_ENDPOINT)?;
        let mut body = json!({"q": query, "num": input.num});
        if let Some(country) = input.country {
            body["gl"] = country.into();
        }
        if let Some(language) = input.language {
            body["hl"] = language.into();
        }
        let response = self
            .outbound
            .client()
            .post(endpoint)
            .header("X-API-KEY", key)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| Error::Upstream(format!("web search transport: {error}")))?;
        let status = response.status();
        let bytes = read_bounded(response, SEARCH_MAX_RESPONSE_BYTES).await?;
        if !status.is_success() {
            return Err(Error::Upstream(format!(
                "web search provider returned {status}: {}",
                safe_preview(&bytes, 512)
            )));
        }
        let provider: Value = serde_json::from_slice(&bytes)
            .map_err(|error| Error::Upstream(format!("web search response: {error}")))?;
        let mut results: Vec<Value> = provider["organic"]
            .as_array()
            .into_iter()
            .flatten()
            .take(input.num)
            .filter_map(|item| {
                let title = item["title"].as_str()?;
                let url = item["link"].as_str()?;
                let mut result = json!({
                    "title": take_chars(title, 512),
                    "url": take_chars(url, 2048),
                    "snippet": take_chars(item["snippet"].as_str().unwrap_or(""), 2000)
                });
                if let Some(date) = item["date"].as_str() {
                    result["date"] = take_chars(date, 64).into();
                }
                Some(result)
            })
            .collect();
        loop {
            let encoded = serde_json::to_string(&json!({"query": query, "results": results}))
                .map_err(|error| Error::Internal(format!("web search result: {error}")))?;
            if encoded.len() <= MAX_RESULT_BYTES {
                return Ok(encoded);
            }
            if results.pop().is_none() {
                return Err(Error::Internal(
                    "web search result could not fit the bounded Tool response".into(),
                ));
            }
        }
    }

    async fn fetch(&self, input: &Value) -> Result<String> {
        let input: FetchInput = serde_json::from_value(input.clone())
            .map_err(|error| Error::Invalid(format!("web_fetch input: {error}")))?;
        let max_chars = input.max_chars.unwrap_or(FETCH_MAX_CHARS);
        if max_chars == 0 || max_chars > FETCH_MAX_CHARS {
            return Err(Error::Invalid(format!(
                "web_fetch.max_chars must be between 1 and {FETCH_MAX_CHARS}"
            )));
        }
        let mut current = self.outbound.check_url(&input.url)?;
        let mut redirects = 0usize;
        let response = loop {
            let response = self
                .outbound
                .client()
                .get(current.clone())
                .header(
                    reqwest::header::ACCEPT,
                    "text/html, text/plain, application/json;q=0.9",
                )
                .send()
                .await
                .map_err(|error| Error::Upstream(format!("web fetch transport: {error}")))?;
            if response.status().is_redirection() {
                if redirects >= MAX_REDIRECTS {
                    return Err(Error::Upstream(format!(
                        "web fetch exceeded {MAX_REDIRECTS} redirects"
                    )));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        Error::Upstream("web fetch redirect has no valid Location".into())
                    })?;
                let next = current
                    .join(location)
                    .map_err(|error| Error::Invalid(format!("web fetch redirect URL: {error}")))?;
                current = self.outbound.check_url(next.as_str())?;
                redirects += 1;
                continue;
            }
            break response;
        };
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or("application/octet-stream")
            .trim()
            .to_ascii_lowercase();
        if !status.is_success() {
            return Err(Error::Upstream(format!(
                "web fetch returned non-success status {status}"
            )));
        }
        let supported = content_type == "text/html"
            || content_type == "text/plain"
            || content_type == "application/json"
            || content_type.ends_with("+json");
        if !supported {
            return Err(Error::Invalid(format!(
                "web_fetch content type {content_type} is not text, HTML, or JSON"
            )));
        }
        let bytes = read_bounded(response, FETCH_MAX_RESPONSE_BYTES).await?;
        let decoded = String::from_utf8_lossy(&bytes);
        let text = if content_type == "text/html" {
            html2text::from_read(decoded.as_bytes(), 120)
                .map_err(|error| Error::Upstream(format!("HTML conversion: {error}")))?
        } else {
            decoded.into_owned()
        };
        encode_fetch_result(
            current.as_str(),
            status.as_u16(),
            &content_type,
            &text,
            max_chars,
        )
    }
}

fn tool_failure(outcome: &str, content: String) -> Value {
    json!({
        "outcome": outcome,
        "content": take_chars(&content, 4096),
        "is_error": true,
        "disposition": "continue"
    })
}

fn safe_error(error: &Error) -> String {
    match error {
        Error::Invalid(message) | Error::Upstream(message) | Error::Internal(message) => {
            take_chars(message, 4096)
        }
        _ => "managed web Tool failed".into(),
    }
}

fn take_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

fn encode_fetch_result(
    url: &str,
    status: u16,
    content_type: &str,
    full_text: &str,
    requested_chars: usize,
) -> Result<String> {
    let available = full_text.chars().count();
    let mut low = 0usize;
    let mut high = available.min(requested_chars);
    let requested_truncation = available > requested_chars;
    let mut best = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        let text = take_chars(full_text, middle);
        let encoded = serde_json::to_string(&json!({
            "url": url,
            "status": status,
            "content_type": content_type,
            "text": text,
            "truncated": requested_truncation || middle < available
        }))
        .map_err(|error| Error::Internal(format!("web fetch result: {error}")))?;
        if encoded.len() <= MAX_RESULT_BYTES {
            best = Some(encoded);
            low = middle.saturating_add(1);
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    best.ok_or_else(|| Error::Internal("web fetch metadata exceeds the Tool result bound".into()))
}

async fn read_bounded(response: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(Error::Invalid(format!(
            "web response exceeds the configured {max_bytes}-byte limit"
        )));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| Error::Upstream(format!("web response body: {error}")))?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(Error::Invalid(format!(
                "web response exceeds the configured {max_bytes}-byte limit"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn safe_preview(bytes: &[u8], maximum: usize) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(maximum)]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(name: &str, capability: &str, input: Value) -> ExternalToolCallRequest {
        serde_json::from_value(json!({
            "session_id": "ses_01HZZZZZZZZZZZZZZZZZZZZZZZ",
            "turn_id": "trn_01HZZZZZZZZZZZZZZZZZZZZZZZ",
            "agent_id": "root",
            "call_id": "call_01HZZZZZZZZZZZZZZZZZZZZZZZ",
            "name": name,
            "input": input,
            "context": {"brain.capability": capability}
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn capability_and_name_must_be_the_pinned_pair() {
        let runtime = WebRuntime::hosted(None);
        assert!(
            runtime
                .execute(request(
                    SEARCH_TOOL_NAME,
                    FETCH_CAPABILITY,
                    json!({"query": "aex"})
                ))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn missing_search_configuration_is_a_normal_tool_error() {
        let response = WebRuntime::hosted(None)
            .execute(request(
                SEARCH_TOOL_NAME,
                SEARCH_CAPABILITY,
                json!({"query": "aex"}),
            ))
            .await
            .unwrap();
        assert_eq!(response["outcome"], "failed");
        assert_eq!(response["disposition"], "continue");
        assert_eq!(response["is_error"], true);
    }

    #[tokio::test]
    async fn internal_fetch_targets_fail_before_network_access() {
        let response = WebRuntime::hosted(None)
            .execute(request(
                FETCH_TOOL_NAME,
                FETCH_CAPABILITY,
                json!({"url": "https://169.254.169.254/latest/meta-data/"}),
            ))
            .await
            .unwrap();
        assert_eq!(response["outcome"], "failed");
        assert!(response["content"].as_str().unwrap().contains("SSRF guard"));
    }

    #[test]
    fn fetch_results_remain_valid_json_under_the_executor_bound() {
        let text = "\u{0000}\"é".repeat(100_000);
        let encoded =
            encode_fetch_result("https://example.com/", 200, "text/plain", &text, 100_000).unwrap();
        assert!(encoded.len() <= MAX_RESULT_BYTES);
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["truncated"], true);
    }
}
