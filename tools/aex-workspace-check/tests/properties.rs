//! Generated properties over the checker itself.
//!
//! Two things must hold for every input, not only the ones a fixture reaches: a
//! rule set that is not a pure function of its inputs cannot be trusted to say
//! the same thing twice about the same workspace, and a counter that does not
//! track the tool output it summarises is a receipt that lies.

use aex_workspace_check::flake::{
    FlakeInput, JUnitReport, NextestList, SourceScan, scan, scan_ignores,
};
use aex_workspace_check::policy::Policy;
use aex_workspace_check::registry::{
    Authorities, PackageKind, PackageRow, Phase, RegistryInput, SourceFindings, build, check,
};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

fn meta(owner: &str, role: &str) -> String {
    format!(
        r#"{{ "owner": "{owner}", "role": "{role}", "artifact": "none",
             "layers": ["unit"], "concerns": ["property"], "seams": [],
             "security_tier": "diagnostic", "risk": ["none"], "scenarios": [],
             "not_applicable": {{ "targets": "awaiting the {owner} stream",
                                  "live_suite": "pure crate; no deployed seam" }} }}"#
    )
}

fn rows(names: &[String]) -> Vec<PackageRow> {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| PackageRow {
            path: format!("crates/{name}"),
            name: name.clone(),
            kind: PackageKind::Cargo,
            raw_meta: (index % 3 != 0).then(|| {
                serde_json::from_str(&meta("regional-domains", "domain"))
                    .expect("the fixture parses")
            }),
            test_targets: Vec::new(),
            features: Vec::new(),
            normal_dependencies: Vec::new(),
        })
        .collect()
}

fn input(packages: Vec<PackageRow>) -> RegistryInput<'static> {
    RegistryInput {
        packages,
        policy: Policy::embedded(),
        workloads: Vec::new(),
        authorities: Authorities::default(),
        source: SourceFindings::default(),
        collected: None,
        phase: Phase::SourceRewrite,
    }
}

proptest! {
    /// The rule set is a pure function of its inputs: the same workspace always
    /// produces the same violations, in the same order.
    #[test]
    fn the_registry_check_is_deterministic(names in prop::collection::vec("aex-[a-z]{3,10}", 1..12)) {
        let unique: Vec<String> = names.into_iter().collect::<BTreeSet<String>>().into_iter().collect();
        let first = check(&input(rows(&unique)));
        let second = check(&input(rows(&unique)));
        prop_assert_eq!(&first.violations, &second.violations);
        prop_assert_eq!(&first.unearned, &second.unearned);
    }

    /// Violations are sorted and free of duplicates, so a diff of two runs is
    /// readable and a rule that fires twice is visible as one row.
    #[test]
    fn violations_are_sorted_and_deduplicated(names in prop::collection::vec("aex-[a-z]{3,10}", 1..12)) {
        let unique: Vec<String> = names.into_iter().collect::<BTreeSet<String>>().into_iter().collect();
        let report = check(&input(rows(&unique)));
        let mut sorted = report.violations.clone();
        sorted.sort();
        sorted.dedup();
        prop_assert_eq!(sorted, report.violations);
    }

    /// The emitted document is deterministic too, which is what makes
    /// `registry verify` a real staleness check rather than a coin toss.
    #[test]
    fn the_registry_document_is_deterministic(names in prop::collection::vec("aex-[a-z]{3,10}", 1..12)) {
        let unique: Vec<String> = names.into_iter().collect::<BTreeSet<String>>().into_iter().collect();
        prop_assert_eq!(build(&input(rows(&unique))), build(&input(rows(&unique))));
    }

    /// Every package with no metadata produces exactly one missing-metadata
    /// violation, whatever else the workspace contains.
    #[test]
    fn one_missing_block_is_one_violation(names in prop::collection::vec("aex-[a-z]{3,10}", 1..12)) {
        let unique: Vec<String> = names.into_iter().collect::<BTreeSet<String>>().into_iter().collect();
        let packages = rows(&unique);
        let undeclared = packages.iter().filter(|row| row.raw_meta.is_none()).count();
        let report = check(&input(packages));
        let reported = report
            .violations
            .iter()
            .filter(|violation| violation.rule == "aex-metadata-missing")
            .count();
        prop_assert_eq!(reported, undeclared);
    }

    /// The declared counter always equals the case count of the list it
    /// summarises: a receipt cannot claim more or fewer cases than nextest saw.
    #[test]
    fn the_declared_counter_tracks_the_list(cases in prop::collection::vec("[a-z_]{3,12}", 0..24)) {
        let unique: Vec<String> = cases.into_iter().collect::<BTreeSet<String>>().into_iter().collect();
        let entries: Vec<String> = unique
            .iter()
            .map(|name| format!(r#""{name}": {{ "ignored": false, "filter-match": {{ "status": "matches" }} }}"#))
            .collect();
        let list = format!(
            r#"{{ "rust-suites": {{ "aex-foo::properties": {{ "kind": "test", "testcases": {{ {} }} }} }} }}"#,
            entries.join(", ")
        );
        let mut junit = String::new();
        for name in &unique {
            use std::fmt::Write as _;
            write!(junit, r#"<testcase name="{name}" classname="aex-foo::properties"/>"#)
                .expect("writing to a String never fails");
        }
        let input = FlakeInput {
            list: NextestList::parse(&list).expect("the fixture parses"),
            junit: JUnitReport::parse(&format!("<testsuites><testsuite>{junit}</testsuite></testsuites>")),
            profile: "ci".to_owned(),
            profile_retries: BTreeMap::from([("ci".to_owned(), 0)]),
            env_retries: None,
            filter: None,
            declared_targets: BTreeMap::new(),
            doctest_crates: BTreeSet::new(),
            doctests: None,
            source: SourceScan::default(),
            rerun: None,
        };
        let report = scan(&input);
        prop_assert_eq!(report.inventory.declared, unique.len());
        prop_assert_eq!(report.inventory.collected, unique.len());
        prop_assert!(report.violations.is_empty(), "{:?}", report.violations);
    }

    /// The ignore scan finds exactly the attributes that were inserted, at the
    /// lines they were inserted on.
    #[test]
    fn the_ignore_scan_finds_every_inserted_attribute(
        before in 0_usize..8,
        count in 0_usize..5,
        after in 0_usize..8,
    ) {
        let attribute = format!("{}]", concat!("#[", "ignore"));
        let mut lines: Vec<String> = (0..before).map(|index| format!("fn before_{index}() {{}}")).collect();
        let inserted: Vec<usize> = (0..count)
            .map(|_| {
                lines.push(attribute.clone());
                lines.len()
            })
            .collect();
        lines.extend((0..after).map(|index| format!("fn after_{index}() {{}}")));
        let hits = scan_ignores("x.rs", &lines.join("\n"));
        prop_assert_eq!(hits.len(), count);
        prop_assert_eq!(
            hits.iter().map(|hit| hit.line).collect::<Vec<usize>>(),
            inserted
        );
    }
}
