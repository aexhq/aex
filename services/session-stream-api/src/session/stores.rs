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
//! no read-only handler state can name it.

use aex_content_aws::object_store::{BucketBinding, S3ContentObjects};
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
    /// The one content object adapter, and therefore the one presigner.
    ///
    /// **This deployable owns exactly one `S3ContentObjects`.** Both the upload
    /// cluster and the registry cluster need a presigner over the same bucket
    /// under the same 300-second lifetime, and a second one would be a second
    /// place for the expiry, the encryption context and the bucket-owner
    /// assertion to drift. `registry_files_download_create`,
    /// `session_files_*_download_create` and the four upload routes all reach S3
    /// through this field and nothing else constructs one.
    pub content_objects: S3ContentObjects,
    /// Read/presign-only session telemetry capability.
    pub session_telemetry: aex_session_telemetry_aws::SessionTelemetryReader,
    /// The physical `session-authority` table name.
    pub session_table: String,
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
            content: ContentStore::new(dynamodb.clone(), config.file_authority_table.clone()),
            registry: RegistryDynamoStore::new(
                dynamodb.clone(),
                config.file_authority_table.clone(),
            ),
            custody: CustodyStore::new(dynamodb.clone(), config.session_table.clone()),
            runtime_activity: RuntimeActivityDynamoStore::new(
                dynamodb.clone(),
                config.runtime_activity_table.clone(),
            ),
            objects: ObjectBinding {
                client: objects.clone(),
                bucket: config.content_bucket.clone(),
                expected_owner: config.content_bucket_owner.clone(),
            },
            content_objects: S3ContentObjects::new(
                objects.clone(),
                BucketBinding {
                    bucket: config.content_bucket.clone(),
                    expected_owner: config.content_bucket_owner.clone(),
                    kms_key_id: config.content_kms_key.value.clone(),
                },
            ),
            session_telemetry: aex_session_telemetry_aws::SessionTelemetryReader::with_exports(
                objects.clone(),
                config.session_telemetry_bucket.clone(),
                config.session_telemetry_kms_key.value.clone(),
            ),
            session_table: config.session_table.clone(),
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
            (
                "regional-authz-projection",
                !self.authz_projection_table.is_empty(),
            ),
            ("content-bucket", !self.objects.bucket.is_empty()),
            (
                "session-telemetry-bucket",
                self.session_telemetry.is_bound(),
            ),
            (
                "content-object-adapter",
                !self.content_objects.binding().kms_key_id.is_empty(),
            ),
        ] {
            if !bound {
                unresolved.push(name.to_owned());
            }
        }
        unresolved
    }
}
