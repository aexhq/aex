//! The janitor's guard, hostilely.
//!
//! OD-35 makes release tests provision in `prd` deliberately, so OD-36 makes
//! reclamation a release gate. A janitor that can delete production resources
//! is a worse defect than the residue it exists to remove, so the guard - not
//! the sweep - is what these cases are about.
//!
//! The safety property, stated once: **the reclaimer is only ever handed a
//! resource that passed every synthetic-test check.** It is enforced by the
//! type system rather than by convention. `Reclaimer::reclaim` takes an
//! `&Admitted`, `Admitted` has private fields and no public constructor, and
//! the only function that returns one is `admit`. No adapter, in this crate or
//! any other, can manufacture one for a resource the guard refused.
//!
//! Every case below hands the janitor an inventory containing at least one
//! resource it must not touch, and asserts both halves: the resource survived,
//! and the refusal says why.

use std::collections::BTreeMap;
use std::sync::Mutex;

use aex_release_tool::admit::Plane;
use aex_release_tool::error::Exit;
use aex_release_tool::janitor::{
    Admitted, DiscoveredResource, Inventory, Reclaimer, RefusalRule, Residue, SweepMode, sweep,
};

const MARKER_KEY: &str = "aex:test-synthetic";
const MARKER_VALUE: &str = "aex-test-harness/v1";
const RUN: &str = "tr_0198f4c2a1b74e8fa0d3c5e6f7089a1b";
const OTHER_RUN: &str = "tr_0198f4c2a1b74e8fa0d3c5e6f7089a2c";

fn at(text: &str) -> time::OffsetDateTime {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .expect("a fixture timestamp is RFC 3339")
}

/// Long after every fixture expiry, so nothing is deferred for being young.
fn now() -> time::OffsetDateTime {
    at("2026-08-01T12:00:00Z")
}

/// Fully and correctly tagged, expired hours ago: the janitor should reclaim it.
fn tagged(kind: &str, identity: &str) -> DiscoveredResource {
    let mut tags = BTreeMap::new();
    tags.insert(MARKER_KEY.to_owned(), MARKER_VALUE.to_owned());
    tags.insert("aex:test-run-id".to_owned(), RUN.to_owned());
    tags.insert("aex:test-owner".to_owned(), "delivery".to_owned());
    tags.insert("aex:test-lane".to_owned(), "e2e".to_owned());
    tags.insert(
        "aex:test-expires-at".to_owned(),
        "2026-08-01T06:00:00Z".to_owned(),
    );
    DiscoveredResource {
        kind: kind.to_owned(),
        identity: identity.to_owned(),
        tags,
    }
}

/// The `aex_minted` kinds must also carry the run prefix in their identity.
fn minted(kind: &str, logical: &str) -> DiscoveredResource {
    tagged(kind, &format!("aextest-{RUN}-{logical}"))
}

fn inventory(resources: Vec<DiscoveredResource>) -> Inventory {
    Inventory {
        schema: "aex.janitor-inventory.v1".to_owned(),
        plane: Plane::Prd,
        resources,
    }
}

/// Records what it was asked to delete. Nothing may reach it that the guard
/// refused.
#[derive(Debug, Default)]
struct SpyReclaimer {
    deleted: Mutex<Vec<String>>,
    refuse: Vec<String>,
}

impl SpyReclaimer {
    fn refusing(identities: &[&str]) -> Self {
        Self {
            deleted: Mutex::new(Vec::new()),
            refuse: identities.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    fn deleted(&self) -> Vec<String> {
        self.deleted.lock().expect("not poisoned").clone()
    }
}

impl Reclaimer for SpyReclaimer {
    fn reclaim(&self, admitted: &Admitted) -> Result<(), String> {
        if self.refuse.iter().any(|id| id == admitted.identity()) {
            return Err("the fixture plane refused the delete".to_owned());
        }
        self.deleted.lock().expect("not poisoned").push(format!(
            "{}:{}",
            admitted.kind(),
            admitted.identity()
        ));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The guard: what the janitor must never touch
// ---------------------------------------------------------------------------

/// The case that matters most. A real production resource carries no AEX tag at
/// all, and the janitor is handed it deliberately - the inventory adapter does
/// not pre-filter, because a guard that is only ever offered safe input is not
/// a guard.
#[test]
fn a_resource_with_no_aex_tags_at_all_is_never_reclaimed() {
    let production = DiscoveredResource {
        kind: "organization".to_owned(),
        identity: "org_customer_0001".to_owned(),
        tags: BTreeMap::new(),
    };
    let spy = SpyReclaimer::default();
    let report = sweep(
        &inventory(vec![production]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert!(
        spy.deleted().is_empty(),
        "the reclaimer was handed an untagged production resource"
    );
    assert_eq!(report.reclaimed.len(), 0);
    assert_eq!(report.refused.len(), 1);
    assert_eq!(report.refused[0].rule, RefusalRule::NotSynthetic);
    assert!(
        report.refused[0].detail.contains("aex:test-synthetic"),
        "{}",
        report.refused[0].detail
    );
    assert!(
        !report.refused[0].is_residue,
        "a resource that is not ours is not our residue"
    );
    assert_eq!(report.residue(), Residue::None);
}

/// The four tags that existed before the marker are not sufficient on their
/// own. A resource carrying only them predates the scheme, or was tagged by
/// something else entirely; either way the janitor does not delete it.
#[test]
fn the_run_id_tag_without_the_marker_is_not_enough_to_be_reclaimed() {
    let mut resource = minted("api_key", "key");
    resource.tags.remove(MARKER_KEY);
    let spy = SpyReclaimer::default();
    let report = sweep(&inventory(vec![resource]), &spy, SweepMode::Reclaim, now());
    assert!(spy.deleted().is_empty());
    assert_eq!(report.refused[0].rule, RefusalRule::NotSynthetic);
}

/// A marker with any other value is a different scheme, or a typo, or someone
/// copying tags between accounts. The comparison is exact.
#[test]
fn a_marker_with_the_wrong_value_is_refused() {
    for wrong in [
        "true",
        "aex-test-harness",
        "aex-test-harness/v2",
        "AEX-TEST-HARNESS/V1",
        "",
        " aex-test-harness/v1",
    ] {
        let mut resource = minted("api_key", "key");
        resource
            .tags
            .insert(MARKER_KEY.to_owned(), wrong.to_owned());
        let spy = SpyReclaimer::default();
        let report = sweep(&inventory(vec![resource]), &spy, SweepMode::Reclaim, now());
        assert!(spy.deleted().is_empty(), "reclaimed on marker `{wrong}`");
        assert_eq!(
            report.refused[0].rule,
            RefusalRule::NotSynthetic,
            "marker `{wrong}`"
        );
    }
}

/// The marker alone is not enough either. For a kind whose name AEX mints, the
/// identity must sit inside the run's own namespace, so forging the tag set
/// onto a production resource still fails: an attacker or a mistake would have
/// to rename it as well.
#[test]
fn a_correctly_tagged_resource_outside_the_run_namespace_is_refused() {
    let mut forged = tagged("api_key", "ak_live_customer_0001");
    forged.tags.insert(
        "aex:test-expires-at".to_owned(),
        "2026-08-01T06:00:00Z".to_owned(),
    );
    let spy = SpyReclaimer::default();
    let report = sweep(&inventory(vec![forged]), &spy, SweepMode::Reclaim, now());
    assert!(
        spy.deleted().is_empty(),
        "a fully tagged production identity was reclaimed"
    );
    assert_eq!(
        report.refused[0].rule,
        RefusalRule::IdentityOutsideRunNamespace
    );
    assert!(
        report.refused[0].detail.contains("aextest-"),
        "{}",
        report.refused[0].detail
    );
}

/// The identity must carry *this* run's prefix, not merely a well-formed one.
#[test]
fn an_identity_carrying_another_runs_prefix_is_refused() {
    let mut crossed = tagged("api_key", &format!("aextest-{OTHER_RUN}-key"));
    crossed
        .tags
        .insert("aex:test-run-id".to_owned(), RUN.to_owned());
    let spy = SpyReclaimer::default();
    let report = sweep(&inventory(vec![crossed]), &spy, SweepMode::Reclaim, now());
    assert!(spy.deleted().is_empty());
    assert_eq!(
        report.refused[0].rule,
        RefusalRule::IdentityOutsideRunNamespace
    );
}

#[test]
fn a_malformed_run_id_is_refused() {
    for bad in ["tr_", "tr_notlowercasehex", "run-1", "", &"tr_a".repeat(20)] {
        let mut resource = minted("api_key", "key");
        resource
            .tags
            .insert("aex:test-run-id".to_owned(), bad.to_owned());
        let spy = SpyReclaimer::default();
        let report = sweep(&inventory(vec![resource]), &spy, SweepMode::Reclaim, now());
        assert!(spy.deleted().is_empty(), "reclaimed on run id `{bad}`");
        assert_eq!(report.refused[0].rule, RefusalRule::RunIdMalformed);
    }
}

#[test]
fn an_unknown_lane_or_owner_is_refused() {
    let mut bad_lane = minted("api_key", "key");
    bad_lane
        .tags
        .insert("aex:test-lane".to_owned(), "production".to_owned());
    let mut bad_owner = minted("api_key", "key2");
    bad_owner
        .tags
        .insert("aex:test-owner".to_owned(), "someone".to_owned());
    let spy = SpyReclaimer::default();
    let report = sweep(
        &inventory(vec![bad_lane, bad_owner]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert!(spy.deleted().is_empty());
    let rules: Vec<RefusalRule> = report.refused.iter().map(|entry| entry.rule).collect();
    assert!(rules.contains(&RefusalRule::LaneUnknown), "{rules:?}");
    assert!(rules.contains(&RefusalRule::OwnerUnknown), "{rules:?}");
}

#[test]
fn an_unparseable_expiry_is_refused_rather_than_treated_as_expired() {
    let mut resource = minted("api_key", "key");
    resource
        .tags
        .insert("aex:test-expires-at".to_owned(), "yesterday".to_owned());
    let spy = SpyReclaimer::default();
    let report = sweep(&inventory(vec![resource]), &spy, SweepMode::Reclaim, now());
    assert!(spy.deleted().is_empty());
    assert_eq!(report.refused[0].rule, RefusalRule::ExpiryUnparseable);
}

#[test]
fn an_unknown_kind_is_refused() {
    let spy = SpyReclaimer::default();
    let report = sweep(
        &inventory(vec![minted("aurora_cluster", "db")]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert!(spy.deleted().is_empty());
    assert_eq!(report.refused[0].rule, RefusalRule::KindUnknown);
}

/// A kind with no discovery route cannot be found by a sweep, so it is refused.
/// Unlike the cases above it *is* residue, because it carries the marker and its
/// run has expired. The two are different facts and the report keeps them apart.
#[test]
fn a_kind_with_no_discovery_route_is_refused_and_counted_as_residue() {
    let spy = SpyReclaimer::default();
    let report = sweep(
        &inventory(vec![minted("sqs_message", "msg")]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert!(spy.deleted().is_empty());
    assert_eq!(report.refused[0].rule, RefusalRule::KindNotReclaimable);
    assert!(
        report.refused[0].is_residue,
        "an expired synthetic resource no sweep can remove is residue"
    );
    assert!(matches!(report.residue(), Residue::Unreclaimed { .. }));
}

// ---------------------------------------------------------------------------
// Concurrency with a live lane
// ---------------------------------------------------------------------------

/// The janitor runs on a schedule and a lane runs when it runs. A run whose
/// deadline has not passed is deferred, never reclaimed, so a sweep can never
/// delete a session out from under a test that is still using it.
#[test]
fn a_run_whose_deadline_has_not_passed_is_deferred_not_reclaimed() {
    let mut live = minted("session", "session");
    live.tags.insert(
        "aex:test-expires-at".to_owned(),
        "2026-08-01T11:50:00Z".to_owned(),
    );
    let spy = SpyReclaimer::default();
    let report = sweep(&inventory(vec![live]), &spy, SweepMode::Reclaim, now());
    assert!(
        spy.deleted().is_empty(),
        "a live lane's session was reclaimed"
    );
    assert_eq!(report.deferred.len(), 1);
    assert_eq!(report.reclaimed.len(), 0);
    assert_eq!(
        report.residue(),
        Residue::None,
        "a run inside its own TTL is not residue"
    );
}

/// The deadline is the expiry plus the policy's grace, so a resource in the
/// grace window is still deferred.
#[test]
fn a_resource_inside_the_residue_grace_window_is_still_deferred() {
    let mut recent = minted("session", "session");
    recent.tags.insert(
        "aex:test-expires-at".to_owned(),
        "2026-08-01T11:40:00Z".to_owned(),
    );
    let spy = SpyReclaimer::default();
    let report = sweep(&inventory(vec![recent]), &spy, SweepMode::Reclaim, now());
    assert!(spy.deleted().is_empty());
    assert_eq!(report.deferred.len(), 1);
    assert!(
        report.deferred[0].deadline.as_str() > "2026-08-01T12:00:00Z",
        "the deadline is the expiry plus the grace period"
    );
}

// ---------------------------------------------------------------------------
// The sweep itself
// ---------------------------------------------------------------------------

#[test]
fn reclamation_happens_in_dependency_order() {
    let spy = SpyReclaimer::default();
    let report = sweep(
        &inventory(vec![
            minted("organization", "org"),
            minted("workspace", "ws"),
            minted("api_key", "key"),
            tagged("payment_customer", "cus_test_0001"),
            tagged("payment_instrument", "pm_test_0001"),
            tagged("auto_recharge_policy", "topup_0001"),
            minted("s3_object", "object"),
            minted("s3_multipart", "upload"),
        ]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert_eq!(report.failed.len(), 0, "{:?}", report.failed);
    let kinds: Vec<&str> = spy
        .deleted()
        .iter()
        .map(|entry| {
            entry
                .split_once(':')
                .expect("the spy records kind:identity")
                .0
                .to_owned()
        })
        .map(|kind| match kind.as_str() {
            "auto_recharge_policy" => "auto_recharge_policy",
            "payment_instrument" => "payment_instrument",
            "api_key" => "api_key",
            "s3_multipart" => "s3_multipart",
            "s3_object" => "s3_object",
            "workspace" => "workspace",
            "organization" => "organization",
            "payment_customer" => "payment_customer",
            other => panic!("unexpected kind `{other}`"),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "auto_recharge_policy",
            "payment_instrument",
            "api_key",
            "s3_multipart",
            "s3_object",
            "workspace",
            "organization",
            "payment_customer",
        ]
    );
    assert_eq!(report.residue(), Residue::None);
}

/// The single worst finding in the review: a money path left an auto-recharge
/// policy enabled and a card attached to a customer nothing deleted. The order
/// is asserted here as its own case because it is the one ordering whose
/// violation keeps costing money after the sweep.
#[test]
fn the_recurring_charge_is_disabled_and_the_card_detached_before_the_customer_goes() {
    let spy = SpyReclaimer::default();
    let report = sweep(
        &inventory(vec![
            tagged("payment_customer", "cus_test_0001"),
            tagged("payment_instrument", "pm_test_0001"),
            tagged("auto_recharge_policy", "topup_0001"),
        ]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert_eq!(report.failed.len(), 0);
    assert_eq!(
        spy.deleted(),
        vec![
            "auto_recharge_policy:topup_0001".to_owned(),
            "payment_instrument:pm_test_0001".to_owned(),
            "payment_customer:cus_test_0001".to_owned(),
        ]
    );
}

#[test]
fn report_mode_reclaims_nothing_and_still_names_everything_it_would_have() {
    let spy = SpyReclaimer::default();
    let report = sweep(
        &inventory(vec![minted("api_key", "key"), minted("session", "session")]),
        &spy,
        SweepMode::Report,
        now(),
    );
    assert!(spy.deleted().is_empty(), "report mode deleted something");
    assert_eq!(report.reclaimable.len(), 2);
    assert_eq!(report.reclaimed.len(), 0);
    assert_eq!(report.mode, SweepMode::Report);
}

/// The janitor is idempotent by construction: it sweeps whatever the plane
/// still holds. A second sweep of a plane the first one emptied finds nothing
/// and succeeds.
#[test]
fn a_second_sweep_of_an_emptied_plane_reclaims_nothing_and_succeeds() {
    let spy = SpyReclaimer::default();
    let first = sweep(
        &inventory(vec![minted("api_key", "key")]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert_eq!(first.reclaimed.len(), 1);
    let second = sweep(&inventory(Vec::new()), &spy, SweepMode::Reclaim, now());
    assert_eq!(second.reclaimed.len(), 0);
    assert_eq!(second.residue(), Residue::None);
    assert_eq!(spy.deleted().len(), 1, "the second sweep deleted again");
}

#[test]
fn a_failed_reclamation_is_reported_with_its_reason_and_counts_as_residue() {
    let spy = SpyReclaimer::refusing(&[&format!("aextest-{RUN}-key")]);
    let report = sweep(
        &inventory(vec![minted("api_key", "key"), minted("session", "session")]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].kind, "api_key");
    assert!(
        report.failed[0].reason.contains("refused the delete"),
        "{}",
        report.failed[0].reason
    );
    assert_eq!(
        report.reclaimed.len(),
        1,
        "one failure does not stop the rest of the sweep"
    );
    match report.residue() {
        Residue::Unreclaimed { count, .. } => assert_eq!(count, 1),
        Residue::None => panic!("expected unreclaimed residue, got none"),
    }
}

/// The hostile mix: one of everything the guard refuses, plus one resource it
/// should reclaim. The reclaimer sees exactly one call.
#[test]
fn on_a_hostile_inventory_the_reclaimer_sees_only_what_the_guard_admitted() {
    let mut untagged = DiscoveredResource {
        kind: "organization".to_owned(),
        identity: "org_customer_0001".to_owned(),
        tags: BTreeMap::new(),
    };
    untagged
        .tags
        .insert("Name".to_owned(), "production".to_owned());
    let mut wrong_marker = minted("workspace", "ws");
    wrong_marker
        .tags
        .insert(MARKER_KEY.to_owned(), "true".to_owned());
    let mut bad_run = minted("session", "session");
    bad_run
        .tags
        .insert("aex:test-run-id".to_owned(), "tr_x".to_owned());
    let forged = tagged("api_key", "ak_live_customer");
    let unknown = minted("aurora_cluster", "db");
    let unreclaimable = minted("sqs_message", "msg");
    let mut live = minted("s3_object", "live");
    live.tags.insert(
        "aex:test-expires-at".to_owned(),
        "2026-08-01T11:55:00Z".to_owned(),
    );
    let good = minted("api_key", "key");

    let spy = SpyReclaimer::default();
    let report = sweep(
        &inventory(vec![
            untagged,
            wrong_marker,
            bad_run,
            forged,
            unknown,
            unreclaimable,
            live,
            good,
        ]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert_eq!(
        spy.deleted(),
        vec![format!("api_key:aextest-{RUN}-key")],
        "the reclaimer must see exactly the one admitted resource"
    );
    assert_eq!(report.reclaimed.len(), 1);
    assert_eq!(report.deferred.len(), 1);
    assert_eq!(report.refused.len(), 6);
    assert_eq!(report.discovered, 8);
}

// ---------------------------------------------------------------------------
// The report the receipt carries
// ---------------------------------------------------------------------------

#[test]
fn a_clean_sweep_reports_no_residue_and_a_dirty_one_reports_why() {
    let spy = SpyReclaimer::default();
    let clean = sweep(
        &inventory(vec![minted("api_key", "key")]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert_eq!(clean.residue(), Residue::None);
    assert_eq!(clean.exit(), Exit::Ok);

    let dirty = sweep(
        &inventory(vec![minted("sqs_message", "msg")]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    assert_eq!(dirty.exit(), Exit::EvidenceUnsound);
    match dirty.residue() {
        Residue::Unreclaimed { count, detail } => {
            assert_eq!(count, 1);
            assert!(detail.contains("sqs_message"), "{detail}");
            assert!(detail.contains(RUN), "{detail}");
        }
        Residue::None => panic!("expected unreclaimed residue, got none"),
    }
}

#[test]
fn the_sweep_report_serializes_to_the_documented_schema() {
    let spy = SpyReclaimer::default();
    let report = sweep(
        &inventory(vec![minted("api_key", "key")]),
        &spy,
        SweepMode::Reclaim,
        now(),
    );
    let value = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(value["schema"], "aex.janitor-sweep.v1");
    assert_eq!(value["plane"], "prd");
    assert_eq!(value["mode"], "reclaim");
    assert_eq!(value["sweptAt"], "2026-08-01T12:00:00Z");
    assert_eq!(value["reclaimed"][0]["kind"], "api_key");
    assert_eq!(value["reclaimed"][0]["runId"], RUN);
    assert_eq!(value["residue"], "none");
}

/// The policy is one document. If the janitor read a second copy of the tag
/// vocabulary, the crate that stamps the tags and the tool that sweeps by them
/// could disagree about what a synthetic resource looks like - and the failure
/// mode of that disagreement is either "reclaims nothing" or "reclaims
/// production".
#[test]
fn the_tag_vocabulary_comes_from_the_one_policy_document() {
    let janitor = &aex_workspace_check::policy::Policy::embedded().janitor;
    assert_eq!(janitor.synthetic_tag, MARKER_KEY);
    assert_eq!(janitor.synthetic_value, MARKER_VALUE);
    assert_eq!(janitor.residue_grace_minutes, 30);
    assert!(
        janitor
            .minted_name_templates
            .iter()
            .any(|template| template.contains("{run_id}")),
        "a name template must carry the run id substitution point"
    );
}
