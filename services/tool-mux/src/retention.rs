//! Trusted complete tool-result retention in the session telemetry bucket.

use std::path::PathBuf;
use std::sync::Arc;

use aex_tool_mux::{
    ExecutorOutput, ReadyHand, ResultRetentionPort, RetainedResult, ToolCallIdentity, ToolHandle,
    ToolMuxFuture,
};
use aex_wire::ids::ContentHash;
use aws_sdk_s3::primitives::ByteStream;
use sha2::{Digest as _, Sha256};

use crate::storage::GuestFileStreamPort;

/// Immutable S3 retention plus exact-generation guest streaming.
pub struct ProductionResultRetention {
    s3: aws_sdk_s3::Client,
    bucket: String,
    expected_owner: String,
    kms_key_id: String,
    guest: Arc<dyn GuestFileStreamPort>,
}

impl ProductionResultRetention {
    /// Binds the immutable session telemetry bucket and guest streamer.
    #[must_use]
    pub fn new(
        s3: aws_sdk_s3::Client,
        bucket: String,
        expected_owner: String,
        kms_key_id: String,
        guest: Arc<dyn GuestFileStreamPort>,
    ) -> Self {
        Self {
            s3,
            bucket,
            expected_owner,
            kms_key_id,
            guest,
        }
    }

    async fn put(
        &self,
        call: &ToolCallIdentity,
        hash: ContentHash,
        bytes: u64,
        body: ByteStream,
        sandbox_path: Option<String>,
    ) -> Result<RetainedResult, String> {
        let key = object_key(call, hash);
        self.s3
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .expected_bucket_owner(&self.expected_owner)
            .server_side_encryption(aws_sdk_s3::types::ServerSideEncryption::AwsKms)
            .ssekms_key_id(&self.kms_key_id)
            .metadata("sha256", hash.to_string())
            .metadata("session", call.session.to_string())
            .body(body)
            .send()
            .await
            .map_err(|_| "tool result retention upload failed".to_owned())?;
        Ok(RetainedResult {
            object_ref: format!("s3://{}/{key}", self.bucket),
            bytes,
            hash,
            sandbox_path,
        })
    }
}

impl ResultRetentionPort for ProductionResultRetention {
    fn retain_inline<'a>(
        &'a self,
        call: &'a ToolCallIdentity,
        body: &'a [u8],
    ) -> ToolMuxFuture<'a, Result<RetainedResult, String>> {
        Box::pin(async move {
            let hash = ContentHash::of(body);
            self.put(
                call,
                hash,
                body.len() as u64,
                ByteStream::from(body.to_vec()),
                None,
            )
            .await
        })
    }

    fn retain_sandbox_file<'a>(
        &'a self,
        call: &'a ToolCallIdentity,
        ready: ReadyHand,
        path: &'a aex_hands_protocol::operation::GuestPath,
        bytes: u64,
        hash: ContentHash,
    ) -> ToolMuxFuture<'a, Result<RetainedResult, String>> {
        Box::pin(async move {
            let staged = self.guest.stream_verified(ready, path, call).await?;
            if staged.ready != ready
                || staged.source != *path
                || staged.bytes != bytes
                || staged.hash != hash
            {
                return Err("sandbox result retention evidence mismatched".to_owned());
            }
            let staging_path = PathBuf::from(&staged.staging_ref);
            let body = ByteStream::from_path(&staging_path)
                .await
                .map_err(|_| "sandbox result staging could not be opened".to_owned())?;
            let result = self
                .put(call, hash, bytes, body, Some(path.as_str().to_owned()))
                .await;
            let _ = tokio::fs::remove_file(staging_path).await;
            result
        })
    }

    fn read_handle<'a>(
        &'a self,
        _handle: &'a ToolHandle,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
        Box::pin(async { Ok(None) })
    }
}

fn object_key(call: &ToolCallIdentity, hash: ContentHash) -> String {
    let identity: [u8; 32] = Sha256::digest(serde_json::to_vec(call).unwrap_or_default()).into();
    format!(
        "sessions/{}/tool-results/{}/{}",
        call.session,
        hex::encode(identity),
        hash
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aex_wire::ids::{
        AgentId, MessageId, OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId,
    };

    #[test]
    fn retained_key_is_stable_and_session_partitioned() {
        let call = ToolCallIdentity {
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(2, [2; 10])),
            session: SessionId::from_uuid7(Uuid7::compose(3, [3; 10])),
            agent: AgentId::from_uuid7(Uuid7::compose(4, [4; 10])),
            message: MessageId::from_uuid7(Uuid7::compose(5, [5; 10])),
            batch: 0,
            call: "call-1".to_owned(),
            attempt: 1,
        };
        let key = object_key(&call, ContentHash::of(b"body"));
        assert_eq!(key, object_key(&call, ContentHash::of(b"body")));
        assert!(key.starts_with(&format!("sessions/{}/tool-results/", call.session)));
    }
}
