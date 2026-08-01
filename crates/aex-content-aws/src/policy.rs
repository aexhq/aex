//! The bucket-policy requirements this adapter depends on, as data.
//!
//! The adapter already issues `If-None-Match: *` on every create and
//! `If-Match: {etag}` on every delete. The policy exists because a rule enforced
//! only in this code protects nothing against a buggy or compromised caller: the
//! service has to refuse an unconditional create and an unconditional delete
//! whoever asks.
//!
//! The infrastructure stream instantiates these; this module owns the
//! requirement and the test that pins it, so a policy that drifts from the
//! adapter is a failing test rather than a silent hole.

/// One `Deny` the content bucket policy must carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredDeny {
    /// The statement id.
    pub sid: &'static str,
    /// The actions it denies.
    pub actions: &'static [&'static str],
    /// The condition operator.
    pub condition_operator: &'static str,
    /// The condition key.
    pub condition_key: &'static str,
    /// The condition value.
    pub condition_value: &'static str,
    /// Why the deny exists.
    pub rationale: &'static str,
}

/// Every `Deny` the content bucket policy must carry.
pub const REQUIRED_DENIES: &[RequiredDeny] = &[
    RequiredDeny {
        sid: "DenyUnconditionalCreate",
        actions: &["s3:PutObject", "s3:CompleteMultipartUpload"],
        condition_operator: "Null",
        condition_key: "s3:if-none-match",
        condition_value: "true",
        rationale: "an overwrite of a content-addressed body must fail closed at the \
                    service, not only in adapter code (D-14)",
    },
    RequiredDeny {
        sid: "DenyUnconditionalDelete",
        actions: &["s3:DeleteObject", "s3:DeleteObjectVersion"],
        condition_operator: "Null",
        condition_key: "s3:if-match",
        condition_value: "true",
        rationale: "a delete must name the exact ETag the sweep decided on (D-13)",
    },
    RequiredDeny {
        sid: "DenyStaleSignature",
        actions: &["s3:*"],
        condition_operator: "NumericGreaterThan",
        condition_key: "s3:signatureAge",
        condition_value: "300000",
        rationale: "a leaked presigned URL must not outlive the grant that authorised \
                    it; OD-17 pins the two to the same 300 seconds",
    },
];

/// The one principal that may delete, expressed as a `NotPrincipal` deny.
pub const DELETE_PRINCIPAL_ROLE: &str = "content-lifecycle-worker";

#[cfg(test)]
mod tests {
    use super::{DELETE_PRINCIPAL_ROLE, REQUIRED_DENIES};
    use crate::object_key::{MAX_SIGNATURE_AGE_MILLIS, PRESIGN_EXPIRY};

    #[test]
    fn the_policy_denies_an_unconditional_create_and_an_unconditional_delete() {
        let sids: Vec<&str> = REQUIRED_DENIES.iter().map(|deny| deny.sid).collect();
        assert!(sids.contains(&"DenyUnconditionalCreate"));
        assert!(sids.contains(&"DenyUnconditionalDelete"));
    }

    #[test]
    fn the_signature_age_deny_matches_the_expiry_the_adapter_signs_with() {
        let deny = REQUIRED_DENIES
            .iter()
            .find(|deny| deny.condition_key == "s3:signatureAge")
            .expect("a signature-age deny");
        assert_eq!(
            deny.condition_value,
            MAX_SIGNATURE_AGE_MILLIS.to_string(),
            "the policy and the adapter must agree, or the last second of a grant \
             is unusable or the first minute after it is not"
        );
        assert_eq!(
            u128::from(MAX_SIGNATURE_AGE_MILLIS),
            PRESIGN_EXPIRY.as_millis()
        );
    }

    #[test]
    fn only_the_lifecycle_role_may_delete_at_all() {
        assert_eq!(DELETE_PRINCIPAL_ROLE, "content-lifecycle-worker");
    }
}
