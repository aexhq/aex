//! Revision-fenced, bounded effective-limit resolution for regional admission.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::projection::WorkspaceProjection;
use aex_session_dynamodb::wire_pending::{ProjectedLimitBundle, ProjectedLimitBundleHead};
use aex_wire::ids::WorkspaceId;
use aex_wire::limits::LimitId;
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::context::EffectiveLimits;

/// Default maximum workspaces retained by one warm edge process.
///
/// Entries contain three machine words plus identity/revision metadata; the
/// fixed bound prevents a tenant-cardinality burst from becoming memory growth.
pub const DEFAULT_LIMIT_CACHE_ENTRIES: usize = 4_096;

/// Why an effective-limit set could not be established.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LimitProjectionError {
    /// The strongly consistent projection read failed or was absent.
    #[error("effective limits are unavailable")]
    Unavailable,
    /// Head, payload or required dimensions did not describe one complete revision.
    #[error("effective limits are incomplete or revision-inconsistent")]
    Incomplete,
    /// A zero-sized cache is not a bounded cache.
    #[error("effective limit cache capacity must be positive")]
    InvalidCapacity,
}

/// Narrow port for the two strong rows admission needs.
#[async_trait]
pub trait LimitBundleProjection: Send + Sync + 'static {
    /// Reads the completeness/revision fence.
    async fn head(&self, workspace: WorkspaceId) -> Result<ProjectedLimitBundleHead, StoreError>;

    /// Reads the complete effective payload.
    async fn bundle(&self, workspace: WorkspaceId) -> Result<ProjectedLimitBundle, StoreError>;
}

#[async_trait]
impl<T: WorkspaceProjection> LimitBundleProjection for T {
    async fn head(&self, workspace: WorkspaceId) -> Result<ProjectedLimitBundleHead, StoreError> {
        self.read_limit_bundle_head(workspace).await
    }

    async fn bundle(&self, workspace: WorkspaceId) -> Result<ProjectedLimitBundle, StoreError> {
        self.read_limit_bundle(workspace).await
    }
}

/// Resolves the workspace limits used by the common finite edge.
#[async_trait]
pub trait LimitResolver: Send + Sync + 'static {
    /// Resolves one complete, current effective set.
    async fn resolve(
        &self,
        workspace: WorkspaceId,
    ) -> Result<EffectiveLimits, LimitProjectionError>;
}

#[derive(Clone, Copy, Debug)]
struct CacheEntry {
    revision: u64,
    limits: EffectiveLimits,
    last_access: u64,
}

#[derive(Debug, Default)]
struct Cache {
    entries: BTreeMap<WorkspaceId, CacheEntry>,
    clock: u64,
}

impl Cache {
    fn tick(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        if self.clock == 0 {
            for entry in self.entries.values_mut() {
                entry.last_access = 0;
            }
            self.clock = 1;
        }
        self.clock
    }

    fn get(&mut self, workspace: WorkspaceId, revision: u64) -> Option<EffectiveLimits> {
        let access = self.tick();
        let entry = self.entries.get_mut(&workspace)?;
        if entry.revision != revision {
            return None;
        }
        entry.last_access = access;
        Some(entry.limits)
    }

    fn insert(
        &mut self,
        workspace: WorkspaceId,
        revision: u64,
        limits: EffectiveLimits,
        max: usize,
    ) {
        let access = self.tick();
        if !self.entries.contains_key(&workspace)
            && self.entries.len() == max
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(workspace, _)| *workspace)
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            workspace,
            CacheEntry {
                revision,
                limits,
                last_access: access,
            },
        );
    }
}

/// Strongly fenced effective-limit reader with a bounded payload cache.
#[derive(Debug)]
pub struct CapacityProjection<P> {
    projection: P,
    cache: Arc<Mutex<Cache>>,
    max_entries: usize,
}

impl<P: Clone> Clone for CapacityProjection<P> {
    fn clone(&self) -> Self {
        Self {
            projection: self.projection.clone(),
            cache: Arc::clone(&self.cache),
            max_entries: self.max_entries,
        }
    }
}

impl<P> CapacityProjection<P> {
    /// Uses the process-wide default entry bound.
    #[must_use]
    pub fn new(projection: P) -> Self {
        Self {
            projection,
            cache: Arc::new(Mutex::new(Cache::default())),
            max_entries: DEFAULT_LIMIT_CACHE_ENTRIES,
        }
    }

    /// Uses an explicit positive entry bound.
    ///
    /// # Errors
    ///
    /// Returns [`LimitProjectionError::InvalidCapacity`] for zero.
    pub fn with_max_entries(
        projection: P,
        max_entries: usize,
    ) -> Result<Self, LimitProjectionError> {
        if max_entries == 0 {
            return Err(LimitProjectionError::InvalidCapacity);
        }
        Ok(Self {
            projection,
            cache: Arc::new(Mutex::new(Cache::default())),
            max_entries,
        })
    }
}

#[async_trait]
impl<P: LimitBundleProjection> LimitResolver for CapacityProjection<P> {
    async fn resolve(
        &self,
        workspace: WorkspaceId,
    ) -> Result<EffectiveLimits, LimitProjectionError> {
        // The head is intentionally never cached. It is the linearization point
        // that prevents a just-reduced override from admitting against old data.
        let mut head = self
            .projection
            .head(workspace)
            .await
            .map_err(|_| LimitProjectionError::Unavailable)?;
        if let Some(limits) = self.cache.lock().await.get(workspace, head.revision) {
            return Ok(limits);
        }

        let bundle = self
            .projection
            .bundle(workspace)
            .await
            .map_err(|_| LimitProjectionError::Unavailable)?;
        if bundle.revision != head.revision {
            // An atomic authority update may linearize between the two reads.
            // One re-head accepts the now-current payload; any other mismatch
            // is corrupt/incomplete and fails closed.
            head = self
                .projection
                .head(workspace)
                .await
                .map_err(|_| LimitProjectionError::Unavailable)?;
            if bundle.revision != head.revision {
                return Err(LimitProjectionError::Incomplete);
            }
        }
        if bundle.workspace != workspace || head.workspace != workspace {
            return Err(LimitProjectionError::Incomplete);
        }
        let limits = effective(&bundle)?;
        self.cache
            .lock()
            .await
            .insert(workspace, bundle.revision, limits, self.max_entries);
        Ok(limits)
    }
}

fn effective(bundle: &ProjectedLimitBundle) -> Result<EffectiveLimits, LimitProjectionError> {
    Ok(EffectiveLimits {
        json_body_bytes: scalar(bundle, LimitId::ApiJsonBody)?,
        otlp_body_bytes: dimension(bundle, LimitId::TelemetryBatch, "encoded_bytes")?,
        query_page_items: dimension(bundle, LimitId::QueryPage, "items")?,
        query_page_bytes: dimension(bundle, LimitId::QueryPage, "serialized_bytes")?,
    })
}

fn scalar(bundle: &ProjectedLimitBundle, id: LimitId) -> Result<usize, LimitProjectionError> {
    let value = bundle
        .limits
        .iter()
        .find(|limit| limit.id == id)
        .and_then(|limit| limit.effective_value.scalar())
        .ok_or(LimitProjectionError::Incomplete)?;
    usize::try_from(value.get()).map_err(|_| LimitProjectionError::Incomplete)
}

fn dimension(
    bundle: &ProjectedLimitBundle,
    id: LimitId,
    name: &str,
) -> Result<usize, LimitProjectionError> {
    let value = bundle
        .limits
        .iter()
        .find(|limit| limit.id == id)
        .and_then(|limit| limit.effective_value.dimension(name))
        .ok_or(LimitProjectionError::Incomplete)?;
    usize::try_from(value.get()).map_err(|_| LimitProjectionError::Incomplete)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aex_session_dynamodb::error::StoreError;
    use aex_session_dynamodb::wire_pending::{ProjectedLimitBundle, ProjectedLimitBundleHead};
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::limits::LimitId;
    use aex_wire::models::{
        EffectiveWorkspaceLimit, LimitMapValue, LimitScalarValue, LimitSource, LimitValue,
    };
    use aex_wire::types::{DecimalU128, Timestamp};
    use async_trait::async_trait;

    use super::{CapacityProjection, LimitBundleProjection, LimitProjectionError, LimitResolver};

    struct FakeProjection {
        states: Mutex<BTreeMap<WorkspaceId, (u64, usize)>>,
        heads: AtomicUsize,
        bundles: AtomicUsize,
    }

    impl FakeProjection {
        fn new(states: impl IntoIterator<Item = (WorkspaceId, (u64, usize))>) -> Self {
            Self {
                states: Mutex::new(states.into_iter().collect()),
                heads: AtomicUsize::new(0),
                bundles: AtomicUsize::new(0),
            }
        }

        fn state(&self, workspace: WorkspaceId) -> (u64, usize) {
            self.states.lock().expect("lock")[&workspace]
        }
    }

    #[async_trait]
    impl LimitBundleProjection for FakeProjection {
        async fn head(
            &self,
            workspace: WorkspaceId,
        ) -> Result<ProjectedLimitBundleHead, StoreError> {
            self.heads.fetch_add(1, Ordering::SeqCst);
            let (revision, _) = self.state(workspace);
            Ok(ProjectedLimitBundleHead {
                workspace,
                revision,
                defaults_revision: 1,
                changed_at: timestamp(),
            })
        }

        async fn bundle(&self, workspace: WorkspaceId) -> Result<ProjectedLimitBundle, StoreError> {
            self.bundles.fetch_add(1, Ordering::SeqCst);
            let (revision, json_bytes) = self.state(workspace);
            Ok(bundle(workspace, revision, json_bytes))
        }
    }

    fn workspace(seed: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }

    fn timestamp() -> Timestamp {
        Timestamp::from_unix_millis(1).expect("timestamp")
    }

    fn bundle(workspace: WorkspaceId, revision: u64, json_bytes: usize) -> ProjectedLimitBundle {
        let row = |id, effective_value| EffectiveWorkspaceLimit {
            changed_at: timestamp(),
            effective_value,
            id,
            revision,
            source: LimitSource::Default,
        };
        ProjectedLimitBundle {
            workspace,
            revision,
            limits: vec![
                row(
                    LimitId::ApiJsonBody,
                    LimitValue::Scalar(LimitScalarValue {
                        value: DecimalU128::new(json_bytes as u128),
                    }),
                ),
                row(
                    LimitId::TelemetryBatch,
                    LimitValue::Map(LimitMapValue {
                        values: BTreeMap::from([(
                            "encoded_bytes".to_owned(),
                            DecimalU128::new(4 * 1_024 * 1_024),
                        )]),
                    }),
                ),
                row(
                    LimitId::QueryPage,
                    LimitValue::Map(LimitMapValue {
                        values: BTreeMap::from([
                            ("items".to_owned(), DecimalU128::new(1_000)),
                            (
                                "serialized_bytes".to_owned(),
                                DecimalU128::new(8 * 1_024 * 1_024),
                            ),
                        ]),
                    }),
                ),
            ],
        }
    }

    #[tokio::test]
    async fn every_request_reads_the_head_and_revision_changes_reload_the_payload() {
        let workspace = workspace(1);
        let projection = FakeProjection::new([(workspace, (1, 65_536))]);
        let resolver = CapacityProjection::with_max_entries(projection, 2).expect("capacity");
        assert_eq!(
            resolver
                .resolve(workspace)
                .await
                .expect("first")
                .json_body_bytes,
            65_536
        );
        assert_eq!(
            resolver
                .resolve(workspace)
                .await
                .expect("hit")
                .json_body_bytes,
            65_536
        );
        assert_eq!(resolver.projection.heads.load(Ordering::SeqCst), 2);
        assert_eq!(resolver.projection.bundles.load(Ordering::SeqCst), 1);

        resolver
            .projection
            .states
            .lock()
            .expect("lock")
            .insert(workspace, (2, 32_768));
        assert_eq!(
            resolver
                .resolve(workspace)
                .await
                .expect("new revision")
                .json_body_bytes,
            32_768
        );
        assert_eq!(resolver.projection.bundles.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn the_entry_bound_evicts_without_ever_skipping_a_head_read() {
        let first = workspace(1);
        let second = workspace(2);
        let projection = FakeProjection::new([(first, (1, 10)), (second, (1, 20))]);
        let resolver = CapacityProjection::with_max_entries(projection, 1).expect("capacity");
        resolver.resolve(first).await.expect("first");
        resolver.resolve(second).await.expect("second");
        resolver.resolve(first).await.expect("first reload");
        assert_eq!(resolver.projection.heads.load(Ordering::SeqCst), 3);
        assert_eq!(resolver.projection.bundles.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_zero_entry_bound_is_refused() {
        let projection = FakeProjection::new([]);
        assert!(matches!(
            CapacityProjection::with_max_entries(projection, 0),
            Err(LimitProjectionError::InvalidCapacity)
        ));
    }

    #[test]
    fn missing_required_dimensions_fail_closed() {
        let workspace = workspace(1);
        let mut incomplete = bundle(workspace, 1, 65_536);
        let page = incomplete
            .limits
            .iter_mut()
            .find(|limit| limit.id == LimitId::QueryPage)
            .expect("query page");
        let LimitValue::Map(value) = &mut page.effective_value else {
            panic!("query page is a map")
        };
        value.values.remove("serialized_bytes");
        assert_eq!(
            super::effective(&incomplete),
            Err(LimitProjectionError::Incomplete)
        );
    }
}
