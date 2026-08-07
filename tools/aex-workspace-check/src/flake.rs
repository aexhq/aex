//! The no-skip, no-retry scanner.
//!
//! Delivery's `evidence verify` fails a receipt whose counters are non-zero.
//! This module produces those counters from real tool output rather than
//! letting a lane self-report them, and adds the source-side scans no receipt
//! can perform: a lane cannot see an `#[ignore]` it never compiled, or a bare
//! environment read that turned an absent prerequisite into an early return.
//!
//! # A note on the needles
//!
//! The patterns below are assembled with `concat!` so the literal forms never
//! appear contiguously in this file. Without that, the scanner reports its own
//! source, and the obvious fix - exempting the file that defines the rule - is
//! exactly the quarantine `Q-FLAKE` removes.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::rules::Violation;

/// The receipt's `inventory` block, produced from tool output.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// Cases `cargo nextest list` declared.
    pub declared: usize,
    /// Cases the `JUnit` report contains.
    pub collected: usize,
    /// Cases the report marked skipped.
    pub skipped: usize,
    /// Cases the list marked ignored.
    pub ignored: usize,
    /// Cases the lane's filter excluded at run time.
    pub filtered_at_runtime: usize,
    /// Cases that were retried.
    pub retried: usize,
    /// Cases that passed on a later attempt.
    pub flaky: usize,
}

/// One case, as `cargo nextest list --message-format json` declares it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ListedCase {
    /// Whether the case carries an ignore attribute.
    #[serde(default)]
    pub ignored: bool,
    /// The lane filter's verdict for this case.
    #[serde(rename = "filter-match", default)]
    pub filter_match: Option<FilterMatch>,
}

/// Whether the lane's filter selected a case.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FilterMatch {
    /// `matches` when selected.
    pub status: String,
}

/// One test binary, as the list declares it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ListedSuite {
    /// `lib`, `test`, `bin` or `bench`.
    #[serde(default)]
    pub kind: String,
    /// The cases the binary contains.
    #[serde(default)]
    pub testcases: BTreeMap<String, ListedCase>,
}

/// The declared inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct NextestList {
    /// Binary id to its suite.
    #[serde(rename = "rust-suites", default)]
    pub rust_suites: BTreeMap<String, ListedSuite>,
}

impl NextestList {
    /// Parses `cargo nextest list --message-format json` output.
    ///
    /// # Errors
    ///
    /// Returns the underlying parse error when the document is not a nextest
    /// listing.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Every declared case as `<binary-id>::<test name>`.
    #[must_use]
    pub fn case_ids(&self) -> BTreeSet<String> {
        self.rust_suites
            .iter()
            .flat_map(|(binary, suite)| {
                suite
                    .testcases
                    .keys()
                    .map(move |case| format!("{binary}::{case}"))
            })
            .collect()
    }
}

/// One collected case.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CollectedCase {
    /// `<binary-id>::<test name>`.
    pub id: String,
    /// Whether the report marked it skipped.
    pub skipped: bool,
    /// Whether the report recorded a flaky or rerun failure.
    pub flaky: bool,
}

/// A parsed `JUnit` report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JUnitReport {
    /// The collected cases, in document order.
    pub cases: Vec<CollectedCase>,
}

impl JUnitReport {
    /// Parses the `JUnit` XML nextest emits.
    ///
    /// Only the four facts the flake rules need are read - case identity, the
    /// `skipped` element and the two rerun elements - so this is a scan rather
    /// than a general XML parser.
    #[must_use]
    pub fn parse(xml: &str) -> Self {
        let mut cases = Vec::new();
        let mut rest = xml;
        while let Some(start) = rest.find("<testcase") {
            let after = &rest[start..];
            let Some(header_end) = after.find('>') else {
                break;
            };
            let header = &after[..header_end];
            let self_closing = header.ends_with('/');
            let body = if self_closing {
                ""
            } else {
                let tail = &after[header_end + 1..];
                tail.find("</testcase>").map_or(tail, |end| &tail[..end])
            };
            let classname = attribute(header, "classname").unwrap_or_default();
            let name = attribute(header, "name").unwrap_or_default();
            cases.push(CollectedCase {
                id: format!("{classname}::{name}"),
                skipped: body.contains("<skipped"),
                flaky: body.contains("<flakyFailure") || body.contains("<rerunFailure"),
            });
            rest = &after[header_end + 1..];
        }
        Self { cases }
    }
}

fn attribute(header: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = header.find(&needle)? + needle.len();
    let rest = header.get(start..)?;
    let end = rest.find('"')?;
    Some(rest.get(..end)?.to_owned())
}

/// What `cargo test --doc` reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctestSummary {
    /// The crates whose doctests ran, by library target name.
    pub crates: BTreeSet<String>,
    /// How many doctests were ignored.
    pub ignored: usize,
}

impl DoctestSummary {
    /// Parses `cargo test --doc` output.
    #[must_use]
    pub fn parse(output: &str) -> Self {
        let mut crates = BTreeSet::new();
        let mut ignored = 0;
        for line in output.lines() {
            if let Some(name) = line.trim().strip_prefix("Doc-tests ") {
                crates.insert(name.trim().to_owned());
            }
            if let Some(position) = line.find(" ignored") {
                let prefix = &line[..position];
                if let Some(number) = prefix
                    .rsplit(';')
                    .next()
                    .and_then(|tail| tail.split_whitespace().next_back()?.parse::<usize>().ok())
                {
                    ignored += number;
                }
            }
        }
        Self { crates, ignored }
    }
}

/// One source-side finding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceHit {
    /// The file, workspace-relative with forward slashes.
    pub path: String,
    /// One-based line number.
    pub line: usize,
    /// What was found, for the message.
    pub detail: String,
}

/// Everything the tree scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceScan {
    /// Ignore attributes in any source file.
    pub ignore_attributes: Vec<SourceHit>,
    /// Bare environment reads inside a test target.
    pub env_reads: Vec<SourceHit>,
    /// Container image literals outside the harness.
    pub image_literals: Vec<String>,
    /// Quarantine or expected-failure files that exist.
    pub quarantine_files: Vec<String>,
    /// Every `services/` and `workers/` Cargo member whose `src/main.rs` was
    /// scanned for the configuration-rejected process event.
    pub deployable_mains: Vec<String>,
    /// The subset of [`SourceScan::deployable_mains`] that never emits it.
    pub silent_config_mains: Vec<String>,
}

const IGNORE_ATTRIBUTE: &str = concat!("#[", "ignore");
const CFG_ATTR: &str = concat!("cfg_", "attr(");
const IGNORE_IN_CFG_ATTR: &str = concat!(" ", "ignore)]");
const ENV_VAR: &str = concat!("env::", "var");
const OPTION_ENV: &str = concat!("option_", "env!");
const REQUIRED_ENV: &str = concat!("required_", "env!");

/// Scans one Rust source file for ignore attributes.
#[must_use]
pub fn scan_ignores(path: &str, text: &str) -> Vec<SourceHit> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            let hit = trimmed.starts_with(IGNORE_ATTRIBUTE)
                || (trimmed.contains(CFG_ATTR) && trimmed.contains(IGNORE_IN_CFG_ATTR));
            hit.then(|| SourceHit {
                path: path.to_owned(),
                line: index + 1,
                detail: trimmed.to_owned(),
            })
        })
        .collect()
}

/// Scans one test-target source file for bare environment reads.
///
/// A read through `aex_test_harness::required_env!` is the sanctioned form and
/// is not a hit: it panics when the value is absent, which is a failure, which
/// is what a missing prerequisite deserves.
#[must_use]
pub fn scan_env_reads(path: &str, text: &str) -> Vec<SourceHit> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if line.contains(REQUIRED_ENV) {
                return None;
            }
            let variable = line
                .contains(ENV_VAR)
                .then(|| read_argument(line, ENV_VAR))
                .or_else(|| {
                    line.contains(OPTION_ENV)
                        .then(|| read_argument(line, OPTION_ENV))
                })?;
            Some(SourceHit {
                path: path.to_owned(),
                line: index + 1,
                detail: variable,
            })
        })
        .collect()
}

fn read_argument(line: &str, needle: &str) -> String {
    line.find(needle)
        .and_then(|start| {
            let rest = &line[start..];
            let open = rest.find('"')? + 1;
            let tail = rest.get(open..)?;
            let close = tail.find('"')?;
            Some(tail.get(..close)?.to_owned())
        })
        .unwrap_or_else(|| "an environment variable".to_owned())
}

/// The context of a diagnostic rerun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rerun {
    /// The receipt the rerun investigates.
    pub receipt_id: String,
    /// The case ids the original attempt failed on, if the record survived.
    pub first_failure: Option<Vec<String>>,
}

/// Everything the flake rules read.
#[derive(Debug, Clone)]
pub struct FlakeInput {
    /// The declared inventory.
    pub list: NextestList,
    /// The collected inventory.
    pub junit: JUnitReport,
    /// The lane's profile name.
    pub profile: String,
    /// Profile name to its configured retry count.
    pub profile_retries: BTreeMap<String, u32>,
    /// `NEXTEST_RETRIES` in the recorded environment, if it was set.
    pub env_retries: Option<String>,
    /// The lane's filter expression, when it passed one.
    pub filter: Option<String>,
    /// Package to the target names its manifest declares.
    pub declared_targets: BTreeMap<String, Vec<String>>,
    /// Library target names of packages that declare layer `unit`.
    pub doctest_crates: BTreeSet<String>,
    /// What `cargo test --doc` reported, when the lane supplied it.
    pub doctests: Option<DoctestSummary>,
    /// What the tree scan found.
    pub source: SourceScan,
    /// The rerun context, when this is a diagnostic rerun.
    pub rerun: Option<Rerun>,
}

/// The result of the scan.
#[derive(Debug, Clone, PartialEq)]
pub struct FlakeReport {
    /// The counters the receipt carries.
    pub inventory: Inventory,
    /// Rules that failed.
    pub violations: Vec<Violation>,
}

/// Runs every flake rule and produces the receipt's `inventory` block.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn scan(input: &FlakeInput) -> FlakeReport {
    let mut violations = Vec::new();
    let declared = input.list.case_ids();
    let collected: BTreeSet<String> = input
        .junit
        .cases
        .iter()
        .map(|case| case.id.clone())
        .collect();

    let ignored = input
        .list
        .rust_suites
        .values()
        .flat_map(|suite| suite.testcases.values())
        .filter(|case| case.ignored)
        .count();
    let filtered_at_runtime = input
        .list
        .rust_suites
        .values()
        .flat_map(|suite| suite.testcases.values())
        .filter(|case| {
            case.filter_match
                .as_ref()
                .is_some_and(|verdict| verdict.status != "matches")
        })
        .count();
    let skipped = input.junit.cases.iter().filter(|case| case.skipped).count();
    let flaky = input.junit.cases.iter().filter(|case| case.flaky).count();

    // 1. declared inventory equals collected inventory
    let missing: Vec<&String> = declared.difference(&collected).collect();
    if !missing.is_empty() {
        let shown: Vec<String> = missing.iter().take(5).map(|id| (*id).clone()).collect();
        violations.push(Violation {
            rule: "flake-inventory-mismatch",
            detail: format!(
                "declared {} cases, collected {}; missing: {}",
                declared.len(),
                collected.len(),
                shown.join(", ")
            ),
        });
    }

    // 2. no ignored case, no skipped case, no case the lane filtered at run time
    for (binary, suite) in &input.list.rust_suites {
        for (case, listed) in &suite.testcases {
            if listed.ignored {
                violations.push(Violation {
                    rule: "flake-skipped-test",
                    detail: format!(
                        "`{binary}::{case}` has ignored=true; a skipped test is a deleted test that still reports as coverage"
                    ),
                });
            }
        }
    }
    for case in &input.junit.cases {
        if case.skipped {
            violations.push(Violation {
                rule: "flake-skipped-test",
                detail: format!(
                    "`{}` reported <skipped/>; a skipped test is a deleted test that still reports as coverage",
                    case.id
                ),
            });
        }
    }

    // 3. no ignore attribute in any source file
    for hit in &input.source.ignore_attributes {
        violations.push(Violation {
            rule: "flake-ignored-attribute",
            detail: format!(
                "{}:{} uses {IGNORE_ATTRIBUTE}]; move the case to tests/live/ or delete it",
                hit.path, hit.line
            ),
        });
    }

    // 4. no bare environment read in a test target
    for hit in &input.source.env_reads {
        violations.push(Violation {
            rule: "flake-self-skip",
            detail: format!(
                "{}:{} reads env `{}` directly; use aex_test_harness::required_env! so an absent prerequisite fails",
                hit.path, hit.line, hit.detail
            ),
        });
    }

    // 5. every declared target collected at least one case
    for (package, targets) in &input.declared_targets {
        for target in targets {
            let binary_id = format!("{package}::{target}");
            let cases = input
                .list
                .rust_suites
                .get(&binary_id)
                .map_or(0, |suite| suite.testcases.len());
            if cases == 0 {
                violations.push(Violation {
                    rule: "flake-empty-target",
                    detail: format!("`{binary_id}` is a declared target and collected 0 cases"),
                });
            }
        }
    }

    // 6. the lane's filter selected something
    if let Some(filter) = &input.filter {
        let selected = declared.len() - filtered_at_runtime;
        if selected == 0 {
            let package = input
                .list
                .rust_suites
                .keys()
                .next()
                .and_then(|binary| binary.split("::").next())
                .unwrap_or("the selected packages")
                .to_owned();
            violations.push(Violation {
                rule: "flake-empty-selection",
                detail: format!(
                    "filter `{filter}` selected 0 of {} cases in `{package}`",
                    declared.len()
                ),
            });
        }
    }

    // 7. the effective profile never retries
    let retries = input.profile_retries.get(&input.profile).copied();
    if let Some(retries) = retries
        && retries > 0
    {
        violations.push(Violation {
            rule: "flake-retry-configured",
            detail: format!(
                "profile `{}` declares retries = {retries}; blocking lanes never retry to green",
                input.profile
            ),
        });
    }
    if let Some(value) = &input.env_retries {
        violations.push(Violation {
            rule: "flake-retry-configured",
            detail: format!(
                "the recorded environment sets NEXTEST_RETRIES={value}; blocking lanes never retry to green"
            ),
        });
    }

    // 8. no flaky pass
    for case in &input.junit.cases {
        if case.flaky {
            violations.push(Violation {
                rule: "flake-flaky-pass",
                detail: format!(
                    "`{}` passed on attempt 2; a flaky pass is a failing receipt",
                    case.id
                ),
            });
        }
    }

    // 9. doctests ran for every package declaring layer unit
    match &input.doctests {
        None => {
            for name in &input.doctest_crates {
                violations.push(Violation {
                    rule: "flake-doctest-missing",
                    detail: format!(
                        "`{name}` declares layer `unit` but no `cargo test --doc` result was supplied"
                    ),
                });
            }
        }
        Some(summary) => {
            for name in &input.doctest_crates {
                if !summary.crates.contains(name) {
                    violations.push(Violation {
                        rule: "flake-doctest-missing",
                        detail: format!(
                            "`{name}` declares layer `unit` but no `cargo test --doc` result was supplied"
                        ),
                    });
                }
            }
            if summary.ignored > 0 {
                violations.push(Violation {
                    rule: "flake-skipped-test",
                    detail: format!(
                        "`cargo test --doc` reported {} ignored doctest(s); a skipped test is a deleted test that still reports as coverage",
                        summary.ignored
                    ),
                });
            }
        }
    }

    // 10. no quarantine or expected-failure file
    for path in &input.source.quarantine_files {
        violations.push(Violation {
            rule: "flake-quarantine-file",
            detail: format!("`{path}` exists; Q-FLAKE removes all release exemptions"),
        });
    }

    // 11. a rerun carries the first failure forward
    if let Some(rerun) = &input.rerun
        && rerun.first_failure.as_ref().is_none_or(Vec::is_empty)
    {
        violations.push(Violation {
            rule: "flake-first-failure-lost",
            detail: format!(
                "diagnostic rerun for receipt {} does not carry the first failure; the original verdict may not be discarded",
                rerun.receipt_id
            ),
        });
    }

    violations.sort();
    violations.dedup();
    FlakeReport {
        inventory: Inventory {
            declared: declared.len(),
            collected: collected.len(),
            skipped,
            ignored,
            filtered_at_runtime,
            retried: flaky,
            flaky,
        },
        violations,
    }
}

/// Reads the retry count of every profile in a nextest configuration.
#[must_use]
pub fn profile_retries(config: &str) -> BTreeMap<String, u32> {
    #[derive(Deserialize)]
    struct Document {
        #[serde(default)]
        profile: BTreeMap<String, Profile>,
    }
    #[derive(Deserialize)]
    struct Profile {
        #[serde(default)]
        retries: Option<u32>,
    }
    toml::from_str::<Document>(config).map_or_else(
        |_| BTreeMap::new(),
        |document| {
            document
                .profile
                .into_iter()
                .filter_map(|(name, profile)| profile.retries.map(|retries| (name, retries)))
                .collect()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{
        DoctestSummary, FlakeInput, JUnitReport, NextestList, Rerun, SourceHit, SourceScan, scan,
        scan_env_reads, scan_ignores,
    };
    use std::collections::{BTreeMap, BTreeSet};

    const LIST: &str = r#"{
      "rust-suites": {
        "aex-foo": { "kind": "lib", "testcases": {
          "rules::tests::a": { "ignored": false, "filter-match": { "status": "matches" } } } },
        "aex-foo::properties": { "kind": "test", "testcases": {
          "journal_fold_determinism": { "ignored": false, "filter-match": { "status": "matches" } } } }
      }
    }"#;

    const JUNIT: &str = r#"<?xml version="1.0"?><testsuites>
      <testsuite name="aex-foo"><testcase name="rules::tests::a" classname="aex-foo" time="0.1"/></testsuite>
      <testsuite name="aex-foo::properties"><testcase name="journal_fold_determinism" classname="aex-foo::properties" time="0.2"/></testsuite>
    </testsuites>"#;

    fn input() -> FlakeInput {
        FlakeInput {
            list: NextestList::parse(LIST).expect("the fixture parses"),
            junit: JUnitReport::parse(JUNIT),
            profile: "ci".to_owned(),
            profile_retries: BTreeMap::from([("ci".to_owned(), 0)]),
            env_retries: None,
            filter: None,
            declared_targets: BTreeMap::from([(
                "aex-foo".to_owned(),
                vec!["properties".to_owned()],
            )]),
            doctest_crates: BTreeSet::from(["aex_foo".to_owned()]),
            doctests: Some(DoctestSummary {
                crates: BTreeSet::from(["aex_foo".to_owned()]),
                ignored: 0,
            }),
            source: SourceScan::default(),
            rerun: None,
        }
    }

    fn details(report: &super::FlakeReport, rule: &str) -> Vec<String> {
        report
            .violations
            .iter()
            .filter(|violation| violation.rule == rule)
            .map(|violation| violation.detail.clone())
            .collect()
    }

    #[test]
    fn a_clean_lane_produces_a_zeroed_inventory_and_no_violation() {
        let report = scan(&input());
        assert!(report.violations.is_empty(), "{:?}", report.violations);
        assert_eq!(report.inventory.declared, 2);
        assert_eq!(report.inventory.collected, 2);
        assert_eq!(report.inventory.skipped, 0);
        assert_eq!(report.inventory.ignored, 0);
        assert_eq!(report.inventory.flaky, 0);
    }

    #[test]
    fn a_compiled_but_unrun_case_is_an_inventory_mismatch() {
        let mut inputs = input();
        inputs.junit.cases.pop();
        let report = scan(&inputs);
        assert_eq!(
            details(&report, "flake-inventory-mismatch"),
            vec![
                "declared 2 cases, collected 1; missing: aex-foo::properties::journal_fold_determinism"
            ]
        );
    }

    #[test]
    fn an_ignored_case_is_reported_with_the_declared_message() {
        let list = LIST.replace(
            r#""journal_fold_determinism": { "ignored": false"#,
            r#""journal_fold_determinism": { "ignored": true"#,
        );
        let mut inputs = input();
        inputs.list = NextestList::parse(&list).expect("the fixture parses");
        let report = scan(&inputs);
        assert!(
            details(&report, "flake-skipped-test").contains(&
                "`aex-foo::properties::journal_fold_determinism` has ignored=true; a skipped test is a deleted test that still reports as coverage".to_owned()),
            "{:?}",
            details(&report, "flake-skipped-test")
        );
        assert_eq!(report.inventory.ignored, 1);
    }

    #[test]
    fn a_skipped_case_in_the_report_is_reported() {
        let junit = JUNIT.replace(
            r#"<testcase name="rules::tests::a" classname="aex-foo" time="0.1"/>"#,
            r#"<testcase name="rules::tests::a" classname="aex-foo" time="0.1"><skipped/></testcase>"#,
        );
        let mut inputs = input();
        inputs.junit = JUnitReport::parse(&junit);
        let report = scan(&inputs);
        assert_eq!(
            details(&report, "flake-skipped-test"),
            vec![
                "`aex-foo::rules::tests::a` reported <skipped/>; a skipped test is a deleted test that still reports as coverage"
            ]
        );
        assert_eq!(report.inventory.skipped, 1);
    }

    #[test]
    fn a_flaky_pass_is_a_failing_receipt() {
        let junit = JUNIT.replace(
            r#"<testcase name="rules::tests::a" classname="aex-foo" time="0.1"/>"#,
            r#"<testcase name="rules::tests::a" classname="aex-foo" time="0.1"><flakyFailure/></testcase>"#,
        );
        let mut inputs = input();
        inputs.junit = JUnitReport::parse(&junit);
        let report = scan(&inputs);
        assert_eq!(
            details(&report, "flake-flaky-pass"),
            vec![
                "`aex-foo::rules::tests::a` passed on attempt 2; a flaky pass is a failing receipt"
            ]
        );
        assert_eq!(report.inventory.flaky, 1);
        assert_eq!(report.inventory.retried, 1);
    }

    #[test]
    fn a_configured_retry_is_reported_from_the_config_and_from_the_environment() {
        let mut inputs = input();
        inputs.profile = "live".to_owned();
        inputs.profile_retries = BTreeMap::from([("live".to_owned(), 1)]);
        inputs.env_retries = Some("2".to_owned());
        let report = scan(&inputs);
        let reported = details(&report, "flake-retry-configured");
        assert!(
            reported.contains(
                &"profile `live` declares retries = 1; blocking lanes never retry to green"
                    .to_owned()
            ),
            "{reported:?}"
        );
        assert_eq!(reported.len(), 2, "{reported:?}");
    }

    #[test]
    fn a_filter_that_selects_nothing_is_an_error() {
        let list = LIST.replace(r#""status": "matches""#, r#""status": "mismatch""#);
        let mut inputs = input();
        inputs.list = NextestList::parse(&list).expect("the fixture parses");
        inputs.junit = JUnitReport::default();
        inputs.filter = Some("binary(smoke)".to_owned());
        inputs.declared_targets = BTreeMap::new();
        let report = scan(&inputs);
        assert_eq!(
            details(&report, "flake-empty-selection"),
            vec!["filter `binary(smoke)` selected 0 of 2 cases in `aex-foo`"]
        );
        assert_eq!(report.inventory.filtered_at_runtime, 2);
    }

    #[test]
    fn a_declared_target_that_collected_nothing_is_reported() {
        let mut inputs = input();
        inputs
            .declared_targets
            .insert("aex-live-site".to_owned(), vec!["smoke".to_owned()]);
        let report = scan(&inputs);
        assert_eq!(
            details(&report, "flake-empty-target"),
            vec!["`aex-live-site::smoke` is a declared target and collected 0 cases"]
        );
    }

    #[test]
    fn a_missing_doctest_result_is_reported_per_package() {
        let mut inputs = input();
        inputs.doctests = None;
        inputs.doctest_crates = BTreeSet::from(["aex_wire".to_owned()]);
        let report = scan(&inputs);
        assert_eq!(
            details(&report, "flake-doctest-missing"),
            vec!["`aex_wire` declares layer `unit` but no `cargo test --doc` result was supplied"]
        );
    }

    #[test]
    fn an_ignored_doctest_is_a_skipped_test() {
        let mut inputs = input();
        inputs.doctests = Some(DoctestSummary {
            crates: BTreeSet::from(["aex_foo".to_owned()]),
            ignored: 3,
        });
        let report = scan(&inputs);
        assert!(
            details(&report, "flake-skipped-test")[0].contains("3 ignored doctest(s)"),
            "{:?}",
            details(&report, "flake-skipped-test")
        );
    }

    #[test]
    fn a_quarantine_file_is_reported() {
        let mut inputs = input();
        inputs.source.quarantine_files = vec![".test-quarantine.json".to_owned()];
        let report = scan(&inputs);
        assert_eq!(
            details(&report, "flake-quarantine-file"),
            vec!["`.test-quarantine.json` exists; Q-FLAKE removes all release exemptions"]
        );
    }

    #[test]
    fn a_rerun_that_lost_the_first_failure_is_reported() {
        let mut inputs = input();
        inputs.rerun = Some(Rerun {
            receipt_id: "7c1f".to_owned(),
            first_failure: None,
        });
        let report = scan(&inputs);
        assert_eq!(
            details(&report, "flake-first-failure-lost"),
            vec![
                "diagnostic rerun for receipt 7c1f does not carry the first failure; the original verdict may not be discarded"
            ]
        );
    }

    #[test]
    fn a_rerun_that_carries_the_first_failure_is_accepted() {
        let mut inputs = input();
        inputs.rerun = Some(Rerun {
            receipt_id: "7c1f".to_owned(),
            first_failure: Some(vec![
                "aex-foo::properties::journal_fold_determinism".to_owned(),
            ]),
        });
        assert!(scan(&inputs).violations.is_empty());
    }

    #[test]
    fn a_source_ignore_attribute_is_found_with_its_line() {
        let text = format!("fn a() {{}}\n{}]\nfn b() {{}}\n", concat!("#[", "ignore"));
        assert_eq!(
            scan_ignores("crates/aex-foo/tests/properties.rs", &text),
            vec![SourceHit {
                path: "crates/aex-foo/tests/properties.rs".to_owned(),
                line: 2,
                detail: format!("{}]", concat!("#[", "ignore")),
            }]
        );
    }

    #[test]
    fn a_conditional_ignore_attribute_is_found() {
        let text = format!(
            "#[{}windows,{}]\nfn a() {{}}\n",
            concat!("cfg_", "attr("),
            concat!(" ", "ignore)")
        );
        assert_eq!(scan_ignores("x.rs", &text).len(), 1, "{text}");
    }

    #[test]
    fn a_bare_environment_read_is_found_and_the_sanctioned_macro_is_not() {
        let bare = format!(
            "let url = std::{}(\"AEX_PG_URL\").unwrap();",
            concat!("env::", "var")
        );
        let hits = scan_env_reads("crates/aex-foo/tests/integration.rs", &bare);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].detail, "AEX_PG_URL");
        let sanctioned = format!(
            "let url = aex_test_harness::{}(\"AEX_PG_URL\");",
            concat!("required_", "env!")
        );
        assert!(scan_env_reads("crates/aex-foo/tests/integration.rs", &sanctioned).is_empty());
    }

    #[test]
    fn a_bare_environment_read_produces_the_declared_message() {
        let mut inputs = input();
        inputs.source.env_reads = vec![SourceHit {
            path: "crates/aex-foo/tests/integration.rs".to_owned(),
            line: 24,
            detail: "AEX_PG_URL".to_owned(),
        }];
        let report = scan(&inputs);
        assert_eq!(
            details(&report, "flake-self-skip"),
            vec![
                "crates/aex-foo/tests/integration.rs:24 reads env `AEX_PG_URL` directly; use aex_test_harness::required_env! so an absent prerequisite fails"
            ]
        );
    }

    #[test]
    fn the_nextest_configuration_retry_counts_are_read() {
        let config = "[profile.ci]\nretries = 0\n[profile.live]\nretries = 2\n";
        let retries = super::profile_retries(config);
        assert_eq!(retries.get("ci"), Some(&0));
        assert_eq!(retries.get("live"), Some(&2));
    }

    #[test]
    fn a_doctest_summary_is_read_from_cargo_output() {
        let output = "   Doc-tests aex_wire\n\nrunning 3 tests\ntest result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n";
        let summary = DoctestSummary::parse(output);
        assert!(summary.crates.contains("aex_wire"));
        assert_eq!(summary.ignored, 1);
    }
}
