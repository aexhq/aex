//! What the guest is handed at boot, and where it lives on disk.
//!
//! Two things live here because two packages need to agree about them and neither
//! may depend on the other's binary: the guest binary reads them, and the image
//! definition writes them into the rootfs.
//!
//! # The run-hook payload is identity only
//!
//! There is no launch ticket and no launch-authority callback. A credential-free
//! guest can call no authority, so there is nothing for a ticket to protect.
//! Forging this payload requires launching a `MicroVM` in AEX's own account;
//! rewriting it from inside the guest breaks only that customer's own session.
//!
//! The field set is **closed** and the decoder refuses an unknown key. That is
//! H-BOUNDARY B4 as a structural fact: no provider key, KMS key, MCP token,
//! journal credential or AEX API key has anywhere to arrive.

use aex_hands_protocol::operation::OperationBounds;
use aex_wire::ids::{ContentHash, GenerationId};
use serde::{Deserialize, Serialize};

/// Environment variable naming the address the agent listens on.
pub const LISTEN_ADDR_VAR: &str = "AEX_HANDS_LISTEN_ADDR";

/// Environment variable naming the operation journal root.
pub const JOURNAL_ROOT_VAR: &str = "AEX_HANDS_JOURNAL_ROOT";

/// Environment variable naming the guest filesystem root.
pub const GUEST_ROOT_VAR: &str = "AEX_HANDS_GUEST_ROOT";

/// Every variable the guest binary requires.
pub const REQUIRED_VARS: [&str; 3] = [LISTEN_ADDR_VAR, JOURNAL_ROOT_VAR, GUEST_ROOT_VAR];

/// The address the rootfs binds the agent to.
///
/// The provider proxy terminates TLS and re-originates in-VM, so the guest never
/// sees TLS and never sees the endpoint auth header. One port, matching the
/// `CreateMicrovmAuthToken` scope: there is no shell port.
pub const LISTEN_ADDR: &str = "0.0.0.0:8080";

/// The operation journal root in the rootfs.
pub const JOURNAL_ROOT: &str = "/var/lib/aex/hands";

/// The guest filesystem root in the rootfs.
pub const GUEST_ROOT: &str = "/workspace";

/// The bounds carried in the run-hook payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunHookBounds {
    /// Largest terminal body.
    pub max_output_bytes: u64,
    /// Largest single frame.
    pub max_frame_bytes: u32,
    /// Wall-clock ceiling.
    pub max_wall_ms: u64,
    /// How many operations may be open at once.
    pub max_concurrent_operations: u16,
}

impl RunHookBounds {
    /// The contract bounds this payload resolves to.
    #[must_use]
    pub const fn resolve(self) -> OperationBounds {
        OperationBounds {
            max_output_bytes: self.max_output_bytes,
            max_frame_bytes: self.max_frame_bytes,
            max_wall_ms: self.max_wall_ms,
            max_concurrent_operations: self.max_concurrent_operations,
        }
    }
}

/// The run-hook payload, as the guest reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunHook {
    /// Payload version.
    pub v: u8,
    /// The exact generation this guest serves, and only this one.
    pub generation: GenerationId,
    /// The exact protocol version. No negotiation, no downgrade.
    pub protocol_version: u32,
    /// The guest root.
    pub root: String,
    /// The compute shape, as its public token.
    pub size: String,
    /// The image's code-artifact digest.
    pub image_digest: ContentHash,
    /// The image's capability layers.
    pub capabilities: Vec<String>,
    /// Bounds the guest may lower but never raise.
    pub bounds: RunHookBounds,
}

/// The sorted, closed key set of a run-hook payload.
pub const RUN_HOOK_KEYS: [&str; 8] = [
    "bounds",
    "capabilities",
    "generation",
    "imageDigest",
    "protocolVersion",
    "root",
    "size",
    "v",
];

/// The capability token the browser executors are gated on.
pub const BROWSER_CAPABILITY: &str = "browser";

impl RunHook {
    /// Whether the pinned image carries the browser capability.
    #[must_use]
    pub fn carries_browser(&self) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability == BROWSER_CAPABILITY)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GUEST_ROOT, JOURNAL_ROOT, LISTEN_ADDR, REQUIRED_VARS, RUN_HOOK_KEYS, RunHook, RunHookBounds,
    };
    use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, Uuid7};

    fn payload() -> RunHook {
        RunHook {
            v: 1,
            generation: GenerationId::from_uuid7(Uuid7::compose(1, [1; 10])),
            protocol_version: 1,
            root: GUEST_ROOT.to_owned(),
            size: "1gb".to_owned(),
            image_digest: ContentHash::from_bytes([7; 32]),
            capabilities: vec!["browser".to_owned()],
            bounds: RunHookBounds {
                max_output_bytes: 1_000_000,
                max_frame_bytes: 1_048_576,
                max_wall_ms: 600_000,
                max_concurrent_operations: 32,
            },
        }
    }

    #[test]
    fn the_payload_key_set_is_closed_and_sorted() {
        let json = serde_json::to_value(payload()).expect("it serializes");
        let object = json.as_object().expect("an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, RUN_HOOK_KEYS);
        assert!(
            RUN_HOOK_KEYS.windows(2).all(|pair| pair[0] < pair[1]),
            "the published key set is sorted so a reviewer can diff it"
        );
    }

    #[test]
    fn a_payload_carrying_anything_else_is_refused() {
        let mut json = serde_json::to_value(payload()).expect("it serializes");
        json.as_object_mut()
            .expect("an object")
            .insert("apiKey".to_owned(), serde_json::json!("sk-live-1"));
        assert!(
            serde_json::from_value::<RunHook>(json).is_err(),
            "a closed field set is how B4 stays true without a review promise"
        );
    }

    #[test]
    fn the_browser_gate_reads_the_capability_layer_and_nothing_else() {
        assert!(payload().carries_browser());
        let mut base = payload();
        base.capabilities.clear();
        assert!(!base.carries_browser());
    }

    #[test]
    fn the_rootfs_locations_are_the_ones_the_image_builds() {
        assert_eq!(LISTEN_ADDR, "0.0.0.0:8080");
        assert_eq!(JOURNAL_ROOT, "/var/lib/aex/hands");
        assert_eq!(GUEST_ROOT, "/workspace");
        assert!(
            !REQUIRED_VARS.iter().any(|name| name.starts_with("AWS_")),
            "the guest holds no credential and reads no AWS variable"
        );
    }
}
