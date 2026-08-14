//! Trusted Tool Mux application: one-Hand readiness, official tools, MCP, storage and bounded live/retained results.
//!
//! Tool Mux is the only component that may combine runtime readiness, the
//! credential-free Hands guest, remote MCP, sandbox-process MCP, storage grants,
//! bounded live previews, and full retained results. Brain stores handles and
//! reads outcomes; it never connects to any executor directly.

mod contract;
mod mux;
mod ports;
mod telemetry;

pub use contract::MAX_LIVE_PREVIEW_BYTES;
pub use contract::{
    EagerPrepareRequest, ExecutorOutput, FullOutput, OfficialSandboxTool, ReadyHand,
    RetainedResult, SandboxConfig, ToolCallIdentity, ToolCompletion, ToolError, ToolHandle,
    ToolHandleRequest, ToolRead, ToolStart, ToolStartRequest, ToolTarget,
};
pub use mux::ToolMux;
pub use ports::{
    GuestPort, McpPort, ResultRetentionPort, RuntimePort, StoragePersistPort, ToolMuxFuture,
};
pub use telemetry::{
    PreparationProgress, TelemetryEnvelope, TelemetryEvent, TelemetryGap, TelemetryKind,
    TelemetryPort, TelemetryPressure, TelemetryProducer,
};

/// Canonical request-body binding signed by Brain and recomputed by ToolMux.
///
/// Both peers operate on the typed internal contract, so the ordinary serde
/// struct encoding is deterministic and includes every admitted request field.
pub fn request_binding<T: serde::Serialize>(request: &T) -> Result<[u8; 32], serde_json::Error> {
    use sha2::Digest as _;

    serde_json::to_vec(request).map(|bytes| sha2::Sha256::digest(bytes).into())
}

/// Parses the release-projected `kid:base64url-public` assertion trust anchors.
///
/// The list is bounded, canonical, duplicate-free, and contains public
/// verification material only. Signing seeds always come from secret custody.
pub fn parse_assertion_trust_anchors(value: &str) -> Result<Vec<(uuid::Uuid, [u8; 32])>, String> {
    use base64::Engine as _;

    let mut anchors = Vec::new();
    for entry in value.split(',') {
        if anchors.len() == 8 {
            return Err("assertion trust anchors exceed the eight-entry bound".to_owned());
        }
        if entry.trim() != entry || entry.is_empty() {
            return Err("assertion trust anchor entries must be non-empty and untrimmed".to_owned());
        }
        let (kid, encoded) = entry
            .split_once(':')
            .ok_or_else(|| "assertion trust anchors use kid:base64url pairs".to_owned())?;
        if encoded.contains(':') {
            return Err("assertion trust anchor pair has an extra colon".to_owned());
        }
        let kid = kid
            .parse::<uuid::Uuid>()
            .map_err(|_| "assertion trust anchor kid is not a UUID".to_owned())?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| "assertion trust anchor key is not base64url".to_owned())?;
        let public = <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| "assertion trust anchor key is not 32 bytes".to_owned())?;
        if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public) != encoded
            || anchors.iter().any(|(existing_kid, existing_public)| {
                *existing_kid == kid || *existing_public == public
            })
        {
            return Err(
                "assertion trust anchors must be canonical and duplicate-free".to_owned(),
            );
        }
        anchors.push((kid, public));
    }
    if anchors.is_empty() {
        return Err("assertion trust anchors must not be empty".to_owned());
    }
    Ok(anchors)
}
