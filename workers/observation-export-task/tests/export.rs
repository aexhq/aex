//! Contract evidence for the one-shot export task.
//!
//! These cases hold the facts a deployment can silently get wrong: what the
//! export task is allowed to hold, what a resume must refuse, what a lost
//! publication means, and which wire code a capacity refusal carries. Every one
//! of them is read from the shared crates or from this deployable's own source
//! rather than restated, so a drift is a failure and not a second copy that
//! agrees with nobody.

use aex_observation_export::checkpoint::{publish, verify_parts};
use aex_observation_export::encoder::encoder_for;
use aex_observation_export::{
    EncodeError, ExportCheckpoint, Format, PartRecord, Publication, PublishFence, ResumeError,
};
use aex_observation_store_dynamodb::composition::{Capability, Role, assert_grant};
use aex_observation_store_dynamodb::health::{HEALTHZ, Probe, READYZ};
use aex_wire::error::ErrorCode;

/// The role this deployable composes as.
const ROLE: Role = Role::ExportTask;

fn checkpoint() -> ExportCheckpoint {
    ExportCheckpoint {
        fence: 3,
        upload_id: "upload".into(),
        parts: vec![
            PartRecord {
                number: 1,
                etag: "a".into(),
                sha256: "0".repeat(64).into(),
                bytes: 5 << 20,
            },
            PartRecord {
                number: 2,
                etag: "b".into(),
                sha256: "1".repeat(64).into(),
                bytes: 5 << 20,
            },
        ],
        cursor: "0|2026-08-01T00:00:00.000Z#abcd1234#3\u{1}".into(),
        member_ordinal: 2,
        member_checkpoint: 40,
        bytes_written: 10 << 20,
    }
}

#[test]
fn the_export_task_holds_no_delete_capability_anywhere() {
    assert_eq!(
        ROLE.granted(),
        &[
            Capability::ReadAuthority,
            Capability::ReadBodies,
            Capability::WriteExportObjects
        ]
    );
    for forbidden in [Capability::DeleteBodies, Capability::DeleteObservations] {
        assert!(
            !ROLE.holds(forbidden),
            "`{}` must not be in the export-task grant",
            forbidden.as_str()
        );
        let violation = assert_grant(ROLE, &[forbidden]).expect_err("composing with it refuses");
        assert_eq!(violation.capability, forbidden);
        assert_eq!(violation.role, ROLE);
    }
}

#[test]
fn the_export_task_can_launch_nothing_and_admit_nothing() {
    for forbidden in [
        Capability::LaunchExportTasks,
        Capability::WriteAdmission,
        Capability::WriteBodies,
        Capability::DeliverUsage,
    ] {
        assert!(!ROLE.holds(forbidden), "{forbidden:?}");
        assert!(assert_grant(ROLE, &[forbidden]).is_err());
    }
    assert_grant(ROLE, ROLE.granted()).expect("its own grant composes");
}

#[test]
fn a_truncated_part_listing_is_a_hard_integrity_failure() {
    // A short list looks exactly like a completed upload, so it can never be
    // taken as an empty tail.
    assert_eq!(
        verify_parts(&checkpoint(), &checkpoint().parts, true),
        Err(ResumeError::TruncatedListing)
    );
    verify_parts(&checkpoint(), &checkpoint().parts, false).expect("a matching listing resumes");

    let short = vec![checkpoint().parts[0].clone()];
    assert!(matches!(
        verify_parts(&checkpoint(), &short, false),
        Err(ResumeError::IntegrityMismatch { .. })
    ));
}

#[test]
fn a_part_the_provider_reports_differently_names_the_part() {
    let mut wrong_size = checkpoint().parts;
    wrong_size[1].bytes = 1;
    assert_eq!(
        verify_parts(&checkpoint(), &wrong_size, false),
        Err(ResumeError::IntegrityMismatch {
            number: 2,
            what: "size"
        })
    );
    let mut wrong_etag = checkpoint().parts;
    wrong_etag[0].etag = "someone-elses".into();
    assert_eq!(
        verify_parts(&checkpoint(), &wrong_etag, false),
        Err(ResumeError::IntegrityMismatch {
            number: 1,
            what: "etag"
        })
    );
}

#[test]
fn losing_the_publication_fence_is_a_supersession_rather_than_a_failure() {
    let healthy = PublishFence {
        state_is_generating: true,
        fence: 3,
        authority_fence: 3,
        cancel_requested: false,
        pinned_deletion_epoch: 1,
        live_deletion_epoch: 1,
    };
    assert_eq!(publish(&healthy), Publication::Ready);
    for mutated in [
        PublishFence {
            cancel_requested: true,
            ..healthy
        },
        PublishFence {
            authority_fence: 4,
            ..healthy
        },
        PublishFence {
            live_deletion_epoch: 2,
            ..healthy
        },
        PublishFence {
            state_is_generating: false,
            ..healthy
        },
    ] {
        assert!(
            matches!(publish(&mutated), Publication::Superseded { .. }),
            "a cancel, a takeover or a deletion is authoritative"
        );
    }
}

#[test]
fn parquet_is_a_typed_refusal_rather_than_a_nondeterministic_artifact() {
    let Err(error) = encoder_for(Format::Parquet) else {
        panic!("parquet must be refused rather than produced");
    };
    assert!(
        matches!(
            error,
            EncodeError::Unsupported {
                format: "parquet",
                ..
            }
        ),
        "{error:?}"
    );
    encoder_for(Format::Ndjson).expect("ndjson is produced");
    encoder_for(Format::OtlpJson).expect("otlp json is produced");
}

#[test]
fn the_capacity_refusal_is_the_registered_code() {
    let code = ErrorCode::ExportCapacity;
    assert_eq!(code.as_str(), "export_capacity");
    assert_eq!(code.http_status(), 503);
    assert!(code.retryable(), "the launcher may re-run a larger task");
}

#[test]
fn the_part_size_floor_is_the_providers_and_is_named_in_the_refusal() {
    let source = include_str!("../src/config.rs");
    assert!(
        source.contains("pub const PART_BYTES_MIN: usize = 5 * 1024 * 1024;"),
        "the floor is the 5 MiB S3 enforces"
    );
    assert!(
        source.contains("S3 refuses a non-final part under 5 MiB"),
        "the refusal explains why a smaller part can never complete"
    );
    assert!(source.contains("AEX_EXPORT_PART_BYTES"));
}

#[test]
fn the_health_paths_are_never_hand_typed() {
    let source = include_str!("../src/health.rs");
    assert!(
        !source.contains("\"/internal/healthz\""),
        "the path is the store crate's constant, not a literal"
    );
    assert!(!source.contains("\"/internal/readyz\""));
    assert!(source.contains("aex_observation_store_dynamodb::health::HEALTHZ"));
    assert_eq!(HEALTHZ, "/internal/healthz");
    assert_eq!(READYZ, "/internal/readyz");
}

#[test]
fn the_declared_probe_set_is_the_table_and_the_bucket() {
    let source = include_str!("../src/main.rs");
    assert!(source.contains("Probe::ObservationTable"));
    assert!(source.contains("Probe::ObservationBucket"));
    assert_eq!(Probe::ObservationTable.as_str(), "observation_table");
    assert_eq!(Probe::ObservationBucket.as_str(), "observation_bucket");
}

#[test]
fn nothing_in_this_deployable_reaches_clickhouse_or_kinesis() {
    let manifest = include_str!("../Cargo.toml").to_ascii_lowercase();
    let sources = [
        include_str!("../src/main.rs"),
        include_str!("../src/aws.rs"),
        include_str!("../src/task.rs"),
        include_str!("../src/budget.rs"),
        include_str!("../src/config.rs"),
        include_str!("../src/health.rs"),
    ];
    for banned in ["clickhouse", "kinesis"] {
        assert!(
            !manifest.contains(banned),
            "`{banned}` must not appear in this deployable's dependency set"
        );
        for source in sources {
            assert!(
                !source.to_ascii_lowercase().contains(banned),
                "`{banned}` must not appear in this deployable's source"
            );
        }
    }
}

#[test]
fn no_case_in_this_deployable_skips_itself() {
    let sources = [
        include_str!("../src/main.rs"),
        include_str!("../src/aws.rs"),
        include_str!("../src/task.rs"),
        include_str!("../src/budget.rs"),
        include_str!("../src/config.rs"),
        include_str!("../src/health.rs"),
        include_str!("./startup.rs"),
        include_str!("./resource_envelope.rs"),
    ];
    for source in sources {
        assert!(!source.contains("#[ignore"), "no case may be ignored");
        assert!(!source.contains("todo!("), "no path may be unimplemented");
        assert!(
            !source.contains("unimplemented!("),
            "no path may be unimplemented"
        );
    }
}
