//! Bounded, replay-safe deletion of immutable Brain checkpoint objects.

use aex_wire::ids::{PrefixedId as _, SessionId};
use aws_sdk_s3::types::{Delete, ObjectIdentifier};

/// One deletion turn never asks S3 to return or delete more than this many
/// checkpoint objects. The operation worker yields after the page.
pub(crate) const CHECKPOINT_DELETE_PAGE_MAX: i32 = 25;

/// The only object namespace owned by session checkpoint cleanup.
#[must_use]
pub(crate) fn checkpoint_prefix(session: SessionId) -> String {
    format!(
        "session-content/v1/session={}/",
        uuid::Uuid::from_bytes(*session.uuid7().as_bytes()).as_hyphenated()
    )
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CheckpointDeletionError {
    #[error("the checkpoint deletion page size is invalid")]
    InvalidPageSize,
    #[error("S3 returned an object outside the exact checkpoint namespace")]
    InvalidStoredKey,
    #[error("checkpoint object storage failed during {operation}")]
    Store { operation: &'static str },
    #[error("S3 reported {failed} failed checkpoint object deletion(s)")]
    PartialDelete { failed: usize },
}

#[derive(Debug)]
struct DeleteOutcome {
    failed: usize,
}

#[async_trait::async_trait]
trait CheckpointObjectStore: Send + Sync {
    async fn list(&self, prefix: &str, limit: i32) -> Result<Vec<String>, CheckpointDeletionError>;

    async fn delete(&self, keys: &[String]) -> Result<DeleteOutcome, CheckpointDeletionError>;
}

/// Existing unversioned customer-content bucket bound to the one lifecycle
/// role that may physically delete immutable objects.
#[derive(Clone, Debug)]
pub(crate) struct S3CheckpointObjects {
    client: aws_sdk_s3::Client,
    bucket: String,
    expected_owner: String,
}

impl S3CheckpointObjects {
    #[must_use]
    pub(crate) fn new(
        client: aws_sdk_s3::Client,
        bucket: impl Into<String>,
        expected_owner: impl Into<String>,
    ) -> Self {
        Self {
            client,
            bucket: bucket.into(),
            expected_owner: expected_owner.into(),
        }
    }

    /// Deletes one bounded page and reports whether a page existed.
    ///
    /// A transport failure is deliberately unresolved: S3 deletion is
    /// idempotent, and the next operation turn re-lists the prefix. A partial
    /// batch is also an error, because only a later empty list may authorize
    /// the session tombstone.
    pub(crate) async fn delete_page(
        &self,
        session: SessionId,
        limit: i32,
    ) -> Result<bool, CheckpointDeletionError> {
        drain_page(self, session, limit).await
    }
}

#[async_trait::async_trait]
impl CheckpointObjectStore for S3CheckpointObjects {
    async fn list(&self, prefix: &str, limit: i32) -> Result<Vec<String>, CheckpointDeletionError> {
        let output = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .expected_bucket_owner(&self.expected_owner)
            .prefix(prefix)
            .max_keys(limit)
            .send()
            .await
            .map_err(|_| CheckpointDeletionError::Store {
                operation: "list_checkpoint_page",
            })?;
        output
            .contents()
            .iter()
            .map(|object| {
                object
                    .key()
                    .map(str::to_owned)
                    .ok_or(CheckpointDeletionError::InvalidStoredKey)
            })
            .collect()
    }

    async fn delete(&self, keys: &[String]) -> Result<DeleteOutcome, CheckpointDeletionError> {
        let objects = keys
            .iter()
            .map(|key| {
                ObjectIdentifier::builder()
                    .key(key)
                    .build()
                    .map_err(|_| CheckpointDeletionError::InvalidStoredKey)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let delete = Delete::builder()
            .set_objects(Some(objects))
            .quiet(true)
            .build()
            .map_err(|_| CheckpointDeletionError::InvalidStoredKey)?;
        let output = self
            .client
            .delete_objects()
            .bucket(&self.bucket)
            .expected_bucket_owner(&self.expected_owner)
            .delete(delete)
            .send()
            .await
            .map_err(|_| CheckpointDeletionError::Store {
                operation: "delete_checkpoint_page",
            })?;
        Ok(DeleteOutcome {
            failed: output.errors().len(),
        })
    }
}

async fn drain_page<S: CheckpointObjectStore + ?Sized>(
    store: &S,
    session: SessionId,
    limit: i32,
) -> Result<bool, CheckpointDeletionError> {
    if !(1..=CHECKPOINT_DELETE_PAGE_MAX).contains(&limit) {
        return Err(CheckpointDeletionError::InvalidPageSize);
    }
    let prefix = checkpoint_prefix(session);
    let keys = store.list(&prefix, limit).await?;
    if keys.len() > usize::try_from(limit).unwrap_or_default() {
        return Err(CheckpointDeletionError::InvalidStoredKey);
    }
    if keys.is_empty() {
        return Ok(false);
    }
    if keys.iter().any(|key| !valid_checkpoint_key(&prefix, key)) {
        return Err(CheckpointDeletionError::InvalidStoredKey);
    }
    let outcome = store.delete(&keys).await?;
    if outcome.failed != 0 {
        return Err(CheckpointDeletionError::PartialDelete {
            failed: outcome.failed,
        });
    }
    Ok(true)
}

fn valid_checkpoint_key(prefix: &str, key: &str) -> bool {
    let Some(tail) = key.strip_prefix(prefix) else {
        return false;
    };
    let mut parts = tail.split('/');
    let Some(agent) = parts.next().and_then(|part| part.strip_prefix("agent=")) else {
        return false;
    };
    let Some(checkpoint) = parts
        .next()
        .and_then(|part| part.strip_prefix("checkpoint="))
    else {
        return false;
    };
    let Some(body) = parts.next().and_then(|part| part.strip_suffix(".json")) else {
        return false;
    };
    parts.next().is_none()
        && valid_hyphenated_uuid(agent)
        && valid_hash(checkpoint)
        && valid_hash(body)
}

fn valid_hyphenated_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, VecDeque};
    use std::sync::Mutex;

    use aex_wire::ids::{PrefixedId as _, Uuid7};

    use super::*;

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10]))
    }

    fn object(index: usize) -> String {
        let agent = format!("00000000-0000-4000-8000-{index:012x}");
        let checkpoint = format!("{index:064x}");
        let body = format!("{:064x}", index.saturating_add(1));
        format!(
            "{}agent={agent}/checkpoint={checkpoint}/{body}.json",
            checkpoint_prefix(session())
        )
    }

    #[derive(Debug, Clone, Copy)]
    enum DeleteFault {
        Partial,
        AmbiguousAfterCommit,
    }

    #[derive(Debug)]
    struct MemoryObjects {
        objects: Mutex<BTreeSet<String>>,
        faults: Mutex<VecDeque<DeleteFault>>,
        listed_limits: Mutex<Vec<i32>>,
    }

    impl MemoryObjects {
        fn new(count: usize) -> Self {
            Self {
                objects: Mutex::new((0..count).map(object).collect()),
                faults: Mutex::new(VecDeque::new()),
                listed_limits: Mutex::new(Vec::new()),
            }
        }

        fn fault(&self, fault: DeleteFault) {
            self.faults.lock().expect("fault lock").push_back(fault);
        }

        fn remaining(&self) -> usize {
            self.objects.lock().expect("object lock").len()
        }
    }

    #[async_trait::async_trait]
    impl CheckpointObjectStore for MemoryObjects {
        async fn list(
            &self,
            prefix: &str,
            limit: i32,
        ) -> Result<Vec<String>, CheckpointDeletionError> {
            self.listed_limits.lock().expect("limit lock").push(limit);
            Ok(self
                .objects
                .lock()
                .expect("object lock")
                .iter()
                .filter(|key| key.starts_with(prefix))
                .take(usize::try_from(limit).unwrap_or_default())
                .cloned()
                .collect())
        }

        async fn delete(&self, keys: &[String]) -> Result<DeleteOutcome, CheckpointDeletionError> {
            let fault = self.faults.lock().expect("fault lock").pop_front();
            let mut objects = self.objects.lock().expect("object lock");
            match fault {
                Some(DeleteFault::Partial) => {
                    for key in keys.iter().skip(1) {
                        objects.remove(key);
                    }
                    Ok(DeleteOutcome { failed: 1 })
                }
                Some(DeleteFault::AmbiguousAfterCommit) => {
                    for key in keys {
                        objects.remove(key);
                    }
                    Err(CheckpointDeletionError::Store {
                        operation: "delete_checkpoint_page",
                    })
                }
                None => {
                    for key in keys {
                        objects.remove(key);
                    }
                    Ok(DeleteOutcome { failed: 0 })
                }
            }
        }
    }

    #[tokio::test]
    async fn partial_listing_and_delete_failure_replay_never_cross_the_page_bound() {
        let store = MemoryObjects::new(61);
        store.fault(DeleteFault::Partial);

        assert!(matches!(
            drain_page(&store, session(), CHECKPOINT_DELETE_PAGE_MAX).await,
            Err(CheckpointDeletionError::PartialDelete { failed: 1 })
        ));
        while drain_page(&store, session(), CHECKPOINT_DELETE_PAGE_MAX)
            .await
            .expect("a replayed checkpoint page")
        {}

        assert_eq!(store.remaining(), 0);
        assert!(
            store
                .listed_limits
                .lock()
                .expect("limit lock")
                .iter()
                .all(|limit| *limit == CHECKPOINT_DELETE_PAGE_MAX)
        );
    }

    #[tokio::test]
    async fn bounded_page_property_holds_across_empty_exact_and_multi_page_populations() {
        for count in 0..=usize::try_from(CHECKPOINT_DELETE_PAGE_MAX).unwrap_or_default() * 4 + 1 {
            let store = MemoryObjects::new(count);
            let mut pages = 0_usize;
            while drain_page(&store, session(), CHECKPOINT_DELETE_PAGE_MAX)
                .await
                .expect("a valid bounded population")
            {
                pages += 1;
            }
            let page_size = usize::try_from(CHECKPOINT_DELETE_PAGE_MAX).unwrap_or_default();
            assert_eq!(pages, count.div_ceil(page_size));
            assert_eq!(store.remaining(), 0);
        }
    }

    #[tokio::test]
    async fn crash_after_s3_commit_is_resolved_by_relisting_before_tombstone() {
        let store = MemoryObjects::new(3);
        store.fault(DeleteFault::AmbiguousAfterCommit);

        assert!(matches!(
            drain_page(&store, session(), CHECKPOINT_DELETE_PAGE_MAX).await,
            Err(CheckpointDeletionError::Store {
                operation: "delete_checkpoint_page"
            })
        ));
        assert_eq!(
            store.remaining(),
            0,
            "S3 may have committed before the crash"
        );
        assert!(
            !drain_page(&store, session(), CHECKPOINT_DELETE_PAGE_MAX)
                .await
                .expect("re-list resolves the ambiguous delete")
        );
    }

    #[test]
    fn only_the_exact_checkpoint_namespace_is_deletable() {
        let prefix = checkpoint_prefix(session());
        assert!(valid_checkpoint_key(&prefix, &object(1)));
        assert!(!valid_checkpoint_key(
            &prefix,
            &format!(
                "{prefix}agent=../checkpoint={}/{}.json",
                "a".repeat(64),
                "b".repeat(64)
            )
        ));
        assert!(!valid_checkpoint_key(
            &prefix,
            &object(1).replace("session-content/v1", "workspace-files/v1")
        ));
    }

    #[tokio::test]
    async fn live_shaped_s3_list_is_owner_bound_prefix_bound_and_bounded() {
        use aws_smithy_http_client::test_util::capture_request;

        let (http, receiver) = capture_request(None);
        let config = aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(aws_sdk_s3::config::Region::new("eu-west-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::for_tests())
            .http_client(http)
            .build();
        let objects = S3CheckpointObjects::new(
            aws_sdk_s3::Client::from_conf(config),
            "aex-dev-content",
            "000000000000",
        );

        assert!(matches!(
            objects
                .delete_page(session(), CHECKPOINT_DELETE_PAGE_MAX)
                .await,
            Err(CheckpointDeletionError::Store {
                operation: "list_checkpoint_page"
            })
        ));
        let request = receiver.expect_request();
        assert_eq!(request.method(), "GET");
        assert_eq!(
            request.headers().get("x-amz-expected-bucket-owner"),
            Some("000000000000")
        );
        let query = request
            .uri()
            .split_once('?')
            .map(|(_, query)| query)
            .expect("an S3 list query");
        assert!(query.contains("list-type=2"), "{query}");
        assert!(query.contains("max-keys=25"), "{query}");
        assert!(
            query.contains("prefix=session-content%2Fv1%2Fsession%3D"),
            "{query}"
        );
    }
}
