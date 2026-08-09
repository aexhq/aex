//! The adapter set the session half composes.
//!
//! The type is the capability statement: every authority this deployable may
//! reach is a field, and nothing else is. There is no queue client and no secret
//! `KMS` key here, which is what makes "the finite API emits no queue message"
//! (RS-02) and "the finite API never decrypts" structural rather than reviewed.
//!
//! Since the stream merged into this process, the type is also the *boundary*:
//! [`Stores`] is the only thing here holding a write handle, it can only be
//! built by a caller holding a `Grant<WorkClaim>`, and the stream half's
//! [`crate::stream::mount::AppState`] has no field that can name it.

use aex_content_dynamodb::store::ContentStore;
use aex_regional_http::capability::{Grant, WorkClaim};
use aex_registry_dynamodb::store::RegistryDynamoStore;
use aex_runtime_activity_dynamodb::store::RuntimeActivityDynamoStore;
use aex_secret_custody_dynamodb::CustodyStore;
use aex_work_dynamodb::store::WorkStore;
use aws_sdk_dynamodb::Client as DynamoDb;
use aws_sdk_s3::Client as ObjectStore;

use crate::config::Config;

/// Every authority this deployable is bound to.
#[derive(Debug, Clone)]
pub struct Stores {
    /// Durable regional continuations.
    pub work: WorkStore,
    /// Content descriptors, edges and pins.
    pub content: ContentStore,
    /// Registered resources and uploads.
    pub registry: RegistryDynamoStore,
    /// Ciphertext metadata. Reads only; this deployable holds no decrypt key.
    pub custody: CustodyStore,
    /// Live workspace generations.
    pub runtime_activity: RuntimeActivityDynamoStore,
    /// The content bucket and the account that must own it.
    pub objects: ObjectBinding,
    /// The physical `session-authority` table name.
    pub session_table: String,
    /// The physical `usage-query-projection` table name.
    pub usage_query_table: String,
    /// The physical `regional-authz-projection` table name.
    pub authz_projection_table: String,
}

/// The object store binding, with the owner every request must assert.
///
/// `ExpectedBucketOwner` is an explicit required input rather than an inferred
/// account: a cross-account bucket confusion has to fail closed.
#[derive(Debug, Clone)]
pub struct ObjectBinding {
    /// The S3 client.
    pub client: ObjectStore,
    /// The content bucket.
    pub bucket: String,
    /// The account that must own it.
    pub expected_owner: String,
}

impl Stores {
    /// Builds every adapter from validated configuration.
    ///
    /// The [`Grant<WorkClaim>`] is unforgeable outside
    /// [`crate::capability::Composition`]. It is what makes the `regional-work`
    /// write handle unconstructible from anywhere but the composition root —
    /// including from a stream handler that knows the table's name, which is the
    /// case the deleted `AEX_WORK_TABLE` refusal used to cover.
    #[must_use]
    pub fn build(
        _grant: &Grant<WorkClaim>,
        config: &Config,
        dynamodb: &DynamoDb,
        objects: &ObjectStore,
    ) -> Self {
        Self {
            work: WorkStore::new(dynamodb.clone(), config.work_table.clone()),
            content: ContentStore::new(dynamodb.clone(), config.content_table.clone()),
            registry: RegistryDynamoStore::new(dynamodb.clone(), config.registry_table.clone()),
            custody: CustodyStore::new(dynamodb.clone(), config.secret_custody_table.clone()),
            runtime_activity: RuntimeActivityDynamoStore::new(
                dynamodb.clone(),
                config.runtime_activity_table.clone(),
            ),
            objects: ObjectBinding {
                client: objects.clone(),
                bucket: config.content_bucket.clone(),
                expected_owner: config.content_bucket_owner.clone(),
            },
            session_table: config.session_table.clone(),
            usage_query_table: config.usage_query_table.clone(),
            authz_projection_table: config.authz_projection_table.clone(),
        }
    }

    /// Dependencies of the served route set that are not yet resolved.
    ///
    /// Readiness is derived from the composition rather than declared: a name
    /// appears here only while something the served routes need is unbound, so
    /// `/internal/readyz` cannot report ready over a half-built process.
    #[must_use]
    pub fn unresolved(&self) -> Vec<String> {
        let mut unresolved = Vec::new();
        for (name, bound) in [
            ("session-authority", !self.session_table.is_empty()),
            ("regional-work", !self.work.table().is_empty()),
            ("regional-content", !self.content.table().is_empty()),
            ("regional-registry", !self.registry.table().is_empty()),
            ("regional-secret-custody", !self.custody.table().is_empty()),
            (
                "runtime-activity",
                !self.runtime_activity.table().is_empty(),
            ),
            ("usage-query-projection", !self.usage_query_table.is_empty()),
            (
                "regional-authz-projection",
                !self.authz_projection_table.is_empty(),
            ),
            ("content-bucket", !self.objects.bucket.is_empty()),
        ] {
            if !bound {
                unresolved.push(name.to_owned());
            }
        }
        unresolved
    }
}
