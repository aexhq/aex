//! Multipart upload identity, part manifests and the integrity argument.
//!
//! S3 supports SHA-256 only as a `COMPOSITE` checksum for a multipart object, so
//! the stored `x-amz-checksum-sha256` is a checksum **of checksums** and is not
//! the whole-object digest a customer sees. Integrity therefore rests on three
//! independent facts rather than one: a per-part SHA-256 verified by S3 on
//! upload, the declared object size supplied as `MpuObjectSize`, and a
//! CRC64NVME full-object checksum that S3 does compute over the whole object.
//!
//! The whole-object re-read and re-hash the previous implementation performed
//! after every completion is deliberately gone (D-12): it doubled egress and
//! latency on every large registration and added no guarantee the three facts
//! above do not already provide. A sampled deep verify runs off the admission
//! path instead.

use aex_wire::ids::ContentHash;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};

use crate::errors::ContentObjectError;
use crate::object_key::ObjectKey;
use crate::redacted::RedactedUrl;

/// The smallest part S3 accepts anywhere but the last position.
pub const MIN_PART_BYTES: u64 = 5 * 1024 * 1024;

/// One live multipart upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartHandle {
    /// The final object key.
    pub key: ObjectKey,
    /// The provider's upload identity.
    pub upload_id: String,
}

/// One part the caller intends to upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartPlan {
    /// The one-based part number.
    pub part_number: i32,
    /// How many bytes it carries.
    pub size: u64,
    /// The part's SHA-256, base64, which S3 verifies on upload.
    pub sha256_base64: String,
}

/// A presigned part upload.
#[derive(Debug, Clone)]
pub struct PresignedPart {
    /// Which part.
    pub part_number: i32,
    /// The URL, which cannot print itself.
    pub url: RedactedUrl,
    /// Headers the caller must replay, including the checksum and length.
    pub headers: Vec<(String, String)>,
    /// How long the URL lives.
    pub expires_in_seconds: u64,
}

/// One part as the provider reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPart {
    /// Which part.
    pub part_number: i32,
    /// How many bytes S3 stored.
    pub size: u64,
    /// The part `ETag`.
    pub etag: String,
    /// The part checksum S3 verified, when the upload declared one.
    pub checksum_sha256: Option<String>,
}

/// Everything a completion asserts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionManifest {
    /// Every part, in ascending order.
    pub parts: Vec<CompletedPartPlan>,
    /// The declared whole-object size, supplied to S3 as `MpuObjectSize`.
    pub total_bytes: u64,
    /// The customer-visible whole-object digest, declared by the client and
    /// verified per part on the way up.
    pub whole_object_sha256: ContentHash,
}

/// One part, with the `ETag` the upload returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedPartPlan {
    /// The plan the caller uploaded.
    pub plan: PartPlan,
    /// The `ETag` the upload returned.
    pub etag: String,
}

impl CompletionManifest {
    /// Checks the manifest against what the provider actually holds.
    ///
    /// # Errors
    ///
    /// [`ContentObjectError::IntegrityMismatch`] when the count, an individual
    /// part number, size, `ETag` or checksum disagrees, or when the sizes do not
    /// add up to the declared total. Each is reported with the part named.
    pub fn agrees_with(&self, provider: &[ProviderPart]) -> Result<(), ContentObjectError> {
        let mismatch = |detail: String| ContentObjectError::IntegrityMismatch { detail };
        if provider.len() != self.parts.len() {
            return Err(mismatch(format!(
                "the manifest declares {} parts and the provider holds {}",
                self.parts.len(),
                provider.len()
            )));
        }
        let mut declared_total = 0_u64;
        for (index, (declared, held)) in self.parts.iter().zip(provider).enumerate() {
            let position = index + 1;
            if declared.plan.part_number != held.part_number {
                return Err(mismatch(format!(
                    "part {position} is number {} in the manifest and {} at the provider",
                    declared.plan.part_number, held.part_number
                )));
            }
            if declared.plan.size != held.size {
                return Err(mismatch(format!(
                    "part {} is {} bytes in the manifest and {} at the provider",
                    declared.plan.part_number, declared.plan.size, held.size
                )));
            }
            if declared.etag != held.etag {
                return Err(mismatch(format!(
                    "part {} has a different ETag at the provider",
                    declared.plan.part_number
                )));
            }
            if held.checksum_sha256.as_deref() != Some(declared.plan.sha256_base64.as_str()) {
                return Err(mismatch(format!(
                    "part {} has no matching SHA-256 at the provider",
                    declared.plan.part_number
                )));
            }
            if position < self.parts.len() && declared.plan.size < MIN_PART_BYTES {
                return Err(mismatch(format!(
                    "part {} is {} bytes, below the {MIN_PART_BYTES} byte minimum",
                    declared.plan.part_number, declared.plan.size
                )));
            }
            declared_total = declared_total.saturating_add(declared.plan.size);
        }
        if declared_total != self.total_bytes {
            return Err(mismatch(format!(
                "the parts add up to {declared_total} bytes and the manifest declares {}",
                self.total_bytes
            )));
        }
        Ok(())
    }

    /// The provider shape of the completion.
    #[must_use]
    pub fn to_completed_upload(&self) -> CompletedMultipartUpload {
        CompletedMultipartUpload::builder()
            .set_parts(Some(
                self.parts
                    .iter()
                    .map(|part| {
                        CompletedPart::builder()
                            .part_number(part.plan.part_number)
                            .e_tag(part.etag.clone())
                            .checksum_sha256(part.plan.sha256_base64.clone())
                            .build()
                    })
                    .collect(),
            ))
            .build()
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::ContentHash;

    use super::{CompletedPartPlan, CompletionManifest, MIN_PART_BYTES, PartPlan, ProviderPart};

    fn manifest() -> CompletionManifest {
        CompletionManifest {
            parts: vec![
                CompletedPartPlan {
                    plan: PartPlan {
                        part_number: 1,
                        size: MIN_PART_BYTES,
                        sha256_base64: "aaa=".to_owned(),
                    },
                    etag: "\"one\"".to_owned(),
                },
                CompletedPartPlan {
                    plan: PartPlan {
                        part_number: 2,
                        size: 128,
                        sha256_base64: "bbb=".to_owned(),
                    },
                    etag: "\"two\"".to_owned(),
                },
            ],
            total_bytes: MIN_PART_BYTES + 128,
            whole_object_sha256: ContentHash::from_bytes([1; 32]),
        }
    }

    fn provider() -> Vec<ProviderPart> {
        vec![
            ProviderPart {
                part_number: 1,
                size: MIN_PART_BYTES,
                etag: "\"one\"".to_owned(),
                checksum_sha256: Some("aaa=".to_owned()),
            },
            ProviderPart {
                part_number: 2,
                size: 128,
                etag: "\"two\"".to_owned(),
                checksum_sha256: Some("bbb=".to_owned()),
            },
        ]
    }

    #[test]
    fn a_manifest_that_matches_the_provider_agrees() {
        manifest().agrees_with(&provider()).expect("agrees");
    }

    #[test]
    fn a_missing_part_is_never_completed_over() {
        let mut short = provider();
        short.pop();
        let error = manifest().agrees_with(&short).expect_err("one part short");
        assert!(error.to_string().contains("2 parts"), "{error}");
    }

    #[test]
    fn a_part_whose_checksum_the_provider_never_verified_is_refused() {
        let mut unverified = provider();
        unverified[1].checksum_sha256 = None;
        let error = manifest()
            .agrees_with(&unverified)
            .expect_err("no provider checksum");
        assert!(error.to_string().contains("part 2"), "{error}");
    }

    #[test]
    fn a_substituted_part_is_caught_by_its_etag() {
        let mut swapped = provider();
        swapped[0].etag = "\"other\"".to_owned();
        assert!(manifest().agrees_with(&swapped).is_err());
    }

    #[test]
    fn parts_that_do_not_add_up_to_the_declared_size_are_refused() {
        let mut declared = manifest();
        declared.total_bytes += 1;
        let error = declared.agrees_with(&provider()).expect_err("size drift");
        assert!(error.to_string().contains("add up to"), "{error}");
    }

    #[test]
    fn an_interior_part_below_the_provider_minimum_is_refused_before_the_service_sees_it() {
        let mut small = manifest();
        small.parts[0].plan.size = 1_024;
        small.total_bytes = 1_024 + 128;
        let mut held = provider();
        held[0].size = 1_024;
        let error = small.agrees_with(&held).expect_err("EntityTooSmall");
        assert!(error.to_string().contains("minimum"), "{error}");
    }

    #[test]
    fn the_completion_carries_every_part_checksum_so_s3_can_verify_the_chain() {
        let upload = manifest().to_completed_upload();
        let parts = upload.parts();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].checksum_sha256(), Some("aaa="));
        assert_eq!(parts[1].part_number(), Some(2));
    }
}
