//! Trusted Tool Mux application: one-Hand readiness, official tools, MCP, storage and bounded live/local results.
//!
//! Tool Mux is the only component that may combine runtime readiness, the
//! credential-free Hands guest, remote MCP, sandbox-process MCP, storage grants,
//! bounded live previews, and full sandbox-local results. Brain stores handles and
//! reads outcomes; it never connects to any executor directly.

mod contract;
mod mux;
mod ports;
mod telemetry;

pub use contract::MAX_LIVE_PREVIEW_BYTES;
pub use contract::{
    ExecutorOutput, FullOutput, OfficialSandboxTool, ReadyHand, SandboxConfig, SandboxResultFile,
    ToolCallIdentity, ToolCompletion, ToolError, ToolHandle, ToolHandleRequest, ToolRead,
    ToolStart, ToolStartRequest, ToolTarget,
};
pub use mux::ToolMux;
pub use ports::{GuestPort, RuntimePort, StoragePersistPort, ToolMuxFuture};
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
pub fn parse_assertion_trust_anchors(value: &str) -> Result<Vec<(uuid::Uuid, [u8; 32])>, ()> {
    use base64::Engine as _;

    let mut anchors = Vec::new();
    for entry in value.split(',') {
        if anchors.len() == 8 || entry.trim() != entry || entry.is_empty() {
            return Err(());
        }
        let (kid, encoded) = entry.split_once(':').ok_or(())?;
        if encoded.contains(':') {
            return Err(());
        }
        let kid = kid.parse::<uuid::Uuid>().map_err(|_| ())?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| ())?;
        let public = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| ())?;
        if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public) != encoded
            || anchors.iter().any(|(existing_kid, existing_public)| {
                *existing_kid == kid || *existing_public == public
            })
        {
            return Err(());
        }
        anchors.push((kid, public));
    }
    if anchors.is_empty() {
        return Err(());
    }
    Ok(anchors)
}
