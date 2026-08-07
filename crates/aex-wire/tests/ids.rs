//! The identifier conformance corpus.
//!
//! The floor is asserted, not assumed: every registry kind must have one
//! accepted case and every rejection mode, and a missing case fails the suite
//! rather than quietly shrinking it.

use std::collections::{BTreeMap, BTreeSet};

use aex_wire::ids::{IdKind, PrefixedId, Uuid7};
use aex_wire::testing::corpus;

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidCase {
    /// The registry key.
    kind: String,
    /// The accepted text.
    text: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InvalidCase {
    /// The registry key.
    kind: String,
    /// The rejected text.
    text: String,
    /// Which rejection mode the case exercises.
    reason: String,
}

/// Parses `text` as the kind named by `key`, returning whether it was accepted.
///
/// This is the one place the 22 concrete newtypes are enumerated; every other
/// test drives them through this function, so adding a kind cannot silently
/// leave a type untested.
fn parse_as(kind: IdKind, text: &str) -> bool {
    use aex_wire::ids::{
        AgentId, ApiKeyId, ApprovalId, ExportId, GenerationId, InvitationId, MeasurementId,
        MembershipId, MessageId, ObservationId, OperationId, OrganizationId, ProviderCredentialId,
        RunId, SessionId, StatementId, TelemetryBatchId, TelemetryGapId, ToolCallId, UploadId,
        UserId, WorkspaceId,
    };
    match kind {
        IdKind::User => UserId::parse(text).is_ok(),
        IdKind::Organization => OrganizationId::parse(text).is_ok(),
        IdKind::Membership => MembershipId::parse(text).is_ok(),
        IdKind::Invitation => InvitationId::parse(text).is_ok(),
        IdKind::Workspace => WorkspaceId::parse(text).is_ok(),
        IdKind::ApiKey => ApiKeyId::parse(text).is_ok(),
        IdKind::ProviderCredential => ProviderCredentialId::parse(text).is_ok(),
        IdKind::Session => SessionId::parse(text).is_ok(),
        IdKind::Message => MessageId::parse(text).is_ok(),
        IdKind::Run => RunId::parse(text).is_ok(),
        IdKind::Agent => AgentId::parse(text).is_ok(),
        IdKind::ToolCall => ToolCallId::parse(text).is_ok(),
        IdKind::Operation => OperationId::parse(text).is_ok(),
        IdKind::Approval => ApprovalId::parse(text).is_ok(),
        IdKind::Generation => GenerationId::parse(text).is_ok(),
        IdKind::Observation => ObservationId::parse(text).is_ok(),
        IdKind::TelemetryBatch => TelemetryBatchId::parse(text).is_ok(),
        IdKind::TelemetryGap => TelemetryGapId::parse(text).is_ok(),
        IdKind::Export => ExportId::parse(text).is_ok(),
        IdKind::Upload => UploadId::parse(text).is_ok(),
        IdKind::Measurement => MeasurementId::parse(text).is_ok(),
        IdKind::Statement => StatementId::parse(text).is_ok(),
    }
}

/// Resolves a registry key to its kind.
fn kind_of(key: &str) -> IdKind {
    IdKind::ALL
        .iter()
        .copied()
        .find(|kind| kind.key() == key)
        .unwrap_or_else(|| panic!("corpus names unknown id kind `{key}`"))
}

#[test]
fn every_registered_kind_has_an_accepted_case() {
    let cases: Vec<ValidCase> = corpus::read_jsonl("ids/valid.jsonl");
    let covered: BTreeSet<&str> = cases.iter().map(|case| case.kind.as_str()).collect();
    for kind in IdKind::ALL {
        assert!(
            covered.contains(kind.key()),
            "`ids/valid.jsonl` has no accepted case for `{}`",
            kind.key()
        );
    }
    for case in &cases {
        let kind = kind_of(&case.kind);
        assert!(
            parse_as(kind, &case.text),
            "`{}` must parse as `{}`",
            case.text,
            case.kind
        );
    }
}

#[test]
fn every_registered_kind_covers_every_rejection_mode() {
    const MODES: [&str; 6] = [
        "wrong_prefix",
        "short_suffix",
        "long_suffix",
        "non_crockford_character",
        "uppercase_suffix",
        "not_uuid_v7",
    ];
    let cases: Vec<InvalidCase> = corpus::read_jsonl("ids/invalid.jsonl");
    let mut seen: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for case in &cases {
        let kind = kind_of(&case.kind);
        assert!(
            !parse_as(kind, &case.text),
            "`{}` must not parse as `{}` ({})",
            case.text,
            case.kind,
            case.reason
        );
        seen.entry(case.kind.as_str())
            .or_default()
            .insert(case.reason.as_str());
    }
    for kind in IdKind::ALL {
        let modes = seen
            .get(kind.key())
            .unwrap_or_else(|| panic!("`{}` has no rejection cases", kind.key()));
        for mode in MODES {
            assert!(
                modes.contains(mode),
                "`{}` has no `{mode}` rejection case",
                kind.key()
            );
        }
    }
}

#[test]
fn an_accepted_id_never_parses_as_another_kind() {
    let cases: Vec<ValidCase> = corpus::read_jsonl("ids/valid.jsonl");
    for case in &cases {
        let owner = kind_of(&case.kind);
        for other in IdKind::ALL.iter().copied() {
            if other == owner {
                continue;
            }
            assert!(
                !parse_as(other, &case.text),
                "`{}` is a `{}` but also parsed as `{}`",
                case.text,
                owner.key(),
                other.key()
            );
        }
    }
}

#[test]
fn encoding_round_trips_and_prefixes_stay_short() {
    use aex_wire::ids::SessionId;
    for kind in IdKind::ALL {
        assert!(
            !kind.prefix().is_empty() && kind.prefix().len() <= 5,
            "`{}` has an unusable prefix",
            kind.key()
        );
        assert!(kind.pattern().starts_with(&format!("^{}_", kind.prefix())));
    }
    for millis in [0_u64, 1, 1_785_501_296_789, (1_u64 << 48) - 1] {
        let value = Uuid7::compose(millis, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(value.unix_millis(), millis);
        let id = SessionId::from_uuid7(value);
        let text = id.encode();
        assert_eq!(SessionId::parse(text.as_str()).expect("round trip"), id);
        assert_eq!(text.as_str().len(), 3 + 1 + 26);
    }
}

/// The Crockford suffix of a `UUIDv7`, as a workspace key segment spells it.
fn suffix(value: Uuid7) -> String {
    String::from_utf8(value.encode_suffix().to_vec()).expect("Crockford is ASCII")
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WorkspaceKeyCase {
    /// What the case exercises.
    reason: String,
    /// Whether the grammar accepts it.
    accepted: bool,
    /// The token.
    text: String,
    /// The region code, for an accepted case.
    #[serde(default)]
    region: Option<String>,
    /// The workspace suffix, for an accepted case.
    #[serde(default)]
    workspace: Option<String>,
    /// The key suffix, for an accepted case.
    #[serde(default)]
    key: Option<String>,
}

#[test]
fn the_workspace_key_grammar_matches_the_shared_corpus() {
    use aex_wire::ids::WorkspaceApiKey;
    let cases: Vec<WorkspaceKeyCase> = corpus::read_jsonl("credentials/workspace-keys.jsonl");
    let mut accepted = 0_usize;
    let mut rejected = 0_usize;
    for case in &cases {
        let parsed = WorkspaceApiKey::parse(&case.text);
        assert_eq!(
            parsed.is_ok(),
            case.accepted,
            "`{}` disagrees with the corpus",
            case.reason
        );
        if let Ok(key) = parsed {
            accepted += 1;
            // The identities are asserted, not just the verdict: two parsers
            // that both "accept" while reading different segments would be a
            // cross-plane authorization bug, not a style difference.
            assert_eq!(
                Some(key.region().code()),
                case.region.as_deref(),
                "{}",
                case.reason
            );
            assert_eq!(
                Some(suffix(key.workspace_id().uuid7())),
                case.workspace.clone(),
                "{}",
                case.reason
            );
            assert_eq!(
                Some(suffix(key.key_id().uuid7())),
                case.key.clone(),
                "{}",
                case.reason
            );
        } else {
            rejected += 1;
        }
    }
    assert!(accepted >= 3 && rejected >= 10, "the corpus has shrunk");
}

#[test]
fn a_workspace_api_key_names_its_workspace_and_never_renders_its_secret() {
    use aex_wire::ids::{ApiKeyId, WorkspaceApiKey, WorkspaceId};
    let workspace = Uuid7::compose(1_785_501_296_789, [1; 10]);
    let key = Uuid7::compose(1_785_501_296_790, [2; 10]);
    let secret = "A".repeat(43);
    let text = format!("aex_wk_euw1_{}_{}_{secret}", suffix(workspace), suffix(key));
    let parsed = WorkspaceApiKey::parse(&text).expect("a well-formed key parses");
    assert_eq!(parsed.region().code(), "euw1");
    assert_eq!(parsed.workspace_id(), WorkspaceId::from_uuid7(workspace));
    assert_eq!(parsed.key_id(), ApiKeyId::from_uuid7(key));
    let rendered = format!("{parsed:?}");
    assert!(rendered.contains("<redacted>"), "{rendered}");
    assert!(!rendered.contains(&secret), "the secret leaked into Debug");
    assert!(
        !rendered.contains(std::str::from_utf8(parsed.secret()).unwrap_or("")),
        "the secret bytes leaked into Debug"
    );

    // A canonical base64url secret legitimately contains `_`; it must never be
    // read as a segment boundary.
    let separated = format!("aex_wk_euw1_{}_{}_{}", suffix(workspace), suffix(key), {
        let mut value = "_".repeat(42);
        value.push('A');
        value
    });
    assert_eq!(
        WorkspaceApiKey::parse(&separated)
            .expect("a secret full of separators parses")
            .key_id(),
        ApiKeyId::from_uuid7(key)
    );

    for bad in [
        // No key segment at all.
        format!("aex_wk_euw1_{}_{secret}", suffix(workspace)),
        // Unknown region.
        format!("aex_wk_zzz9_{}_{}_{secret}", suffix(workspace), suffix(key)),
        // Workspace segment is not a Crockford suffix.
        format!("aex_wk_euw1_notasuffix_{}_{secret}", suffix(key)),
        // Key segment is not a Crockford suffix.
        format!("aex_wk_euw1_{}_notasuffix_{secret}", suffix(workspace)),
        // Secret is the wrong length.
        format!("aex_wk_euw1_{}_{}_short", suffix(workspace), suffix(key)),
        // The old prelaunch spelling, which named no workspace.
        format!("aex_wk_euw1_{}_{secret}", suffix(key)),
        suffix(key),
    ] {
        assert!(
            WorkspaceApiKey::parse(&bad).is_err(),
            "{bad} must not parse"
        );
    }
}
