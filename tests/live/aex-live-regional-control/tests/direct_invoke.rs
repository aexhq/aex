//! Producer/consumer envelope compatibility without AWS credentials.

use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::control::{RegionalControlEnvelope, RegionalControlRequest};
use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::types::Region;

#[test]
fn the_direct_invoke_payload_the_central_consumer_emits_is_the_producer_input() {
    let request = RegionalControlEnvelope {
        schema_version: SchemaVersion::V1,
        request_id: Uuid7::compose(1, [3; 10]),
        payload: RegionalControlRequest::ProvisionWorkspace {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            region: Region::EuWest1,
            fence: 7,
            intent_hash: "ab".repeat(32),
        },
    };
    let bytes = serde_json::to_vec(&request).expect("consumer encodes");
    let decoded: RegionalControlEnvelope<RegionalControlRequest> =
        serde_json::from_slice(&bytes).expect("producer decodes");
    assert_eq!(decoded, request);
}
