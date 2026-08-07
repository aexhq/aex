//! A synthetic repository builder for the graph tests.
//!
//! Fixtures are built rather than checked in wholesale so each test can state
//! exactly the one thing it is about. `cargo metadata` output is synthesized
//! and written to `cargo-metadata.json`, which `GraphInputs::load` prefers over
//! shelling out — the tests stay hermetic and fast, and the parser under test
//! is the same one production uses.

#![allow(dead_code)]

pub mod docs;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// One synthetic Cargo member.
pub struct CratePlan {
    pub name: String,
    pub dir: String,
    pub deps: Vec<(String, Option<&'static str>)>,
    pub meta: Option<Value>,
    pub publish: Option<Value>,
}

impl CratePlan {
    pub fn new(name: &str, dir: &str) -> Self {
        Self {
            name: name.to_owned(),
            dir: dir.to_owned(),
            deps: Vec::new(),
            meta: Some(default_meta("delivery", "domain")),
            publish: Some(json!([])),
        }
    }

    #[must_use]
    pub fn dep(mut self, name: &str) -> Self {
        self.deps.push((name.to_owned(), None));
        self
    }

    #[must_use]
    pub fn dev_dep(mut self, name: &str) -> Self {
        self.deps.push((name.to_owned(), Some("dev")));
        self
    }

    #[must_use]
    pub fn meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }

    #[must_use]
    pub fn without_meta(mut self) -> Self {
        self.meta = None;
        self
    }

    #[must_use]
    pub fn publishable(mut self) -> Self {
        self.publish = None;
        self
    }
}

/// A default ownership block that passes every closed-value-set check.
pub fn default_meta(owner: &str, role: &str) -> Value {
    json!({
        "owner": owner,
        "role": role,
        "artifact": "none",
        "layers": ["unit"],
        "concerns": ["property"],
        "seams": [],
        "security_tier": "internal",
        "risk": ["none"],
        "scenarios": [],
        "targets": { "properties": "unit" },
        "not_applicable": {
            "live_suite": "synthetic fixture crate; it has no deployed seam"
        }
    })
}

/// A live-companion ownership block.
pub fn live_meta(deployable: &str) -> Value {
    live_meta_for(deployable, "SC-DEMO")
}

/// A live-companion ownership block claiming one runnable release scenario.
pub fn live_meta_for(deployable: &str, scenario: &str) -> Value {
    json!({
        "owner": "delivery",
        "role": "live_companion",
        "artifact": "none",
        "deployable": deployable,
        "layers": ["smoke", "e2e"],
        "concerns": ["fault"],
        "seams": [],
        "security_tier": "internal",
        "risk": ["none"],
        "scenarios": [scenario],
        "targets": { "smoke": "smoke" }
    })
}

/// A deployable ownership block.
pub fn deployable_meta(deployable: &str, live_suite: &str) -> Value {
    json!({
        "owner": "delivery",
        "role": "deployable",
        "artifact": "lambda_zip",
        "deployable": deployable,
        "live_suite": live_suite,
        "layers": ["unit", "smoke"],
        "concerns": ["contract", "fault"],
        "seams": [],
        "security_tier": "internal",
        "risk": ["none"],
        "scenarios": [],
        "targets": { "config": "unit" }
    })
}

/// One synthetic npm member.
pub struct NpmPlan {
    pub name: String,
    pub dir: String,
    pub meta: Option<Value>,
}

impl NpmPlan {
    pub fn new(name: &str, dir: &str) -> Self {
        Self {
            name: name.to_owned(),
            dir: dir.to_owned(),
            meta: Some(default_meta("delivery", "tool")),
        }
    }

    #[must_use]
    pub fn meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }
}

/// A deployable ownership block for an npm member, whose `layers` name the
/// evidence a `TypeScript` Lambda actually earns.
pub fn npm_deployable_meta(deployable: &str, live_suite: &str) -> Value {
    json!({
        "owner": "central-finance",
        "role": "deployable",
        "artifact": "lambda_zip",
        "deployable": deployable,
        "live_suite": live_suite,
        "layers": ["unit", "smoke"],
        "concerns": ["contract", "security"],
        "seams": [],
        "security_tier": "secret",
        "risk": ["untrusted_input"],
        "scenarios": [],
        "targets": { "edge": "unit" }
    })
}

/// The fixture under construction.
pub struct Fixture {
    root: tempfile::TempDir,
    crates: Vec<CratePlan>,
    npm: Vec<NpmPlan>,
    npm_workspaces: Vec<String>,
    files: Vec<String>,
    units: String,
    scenarios: String,
    path_map: String,
}

/// The path map every fixture starts with. It classifies the synthesized
/// metadata file so the fixture itself is never an orphan.
pub const FIXTURE_PATH_MAP: &str = r#"
schema = "aex.path-map.v1"

[[rule]]
id = "router"
prefix = "release/"
kind = "router"

[[rule]]
id = "cargo-crates"
prefix = "crates/"
kind = "derive-cargo"

[[rule]]
id = "cargo-services"
prefix = "services/"
kind = "derive-cargo"

[[rule]]
id = "cargo-live"
prefix = "tests/live/"
kind = "derive-cargo"

[[rule]]
id = "npm-explicit-command-edge"
prefix = "services/stripe-command-edge/"
kind = "node"
node = "npm:@fixture/stripe-command-edge"

[[rule]]
id = "npm-packages"
prefix = "packages/"
kind = "derive-npm"

[[rule]]
id = "terraform-modules"
prefix = "infra/modules/"
kind = "derive-terraform"

[[rule]]
id = "lockfile"
exact = "Cargo.lock"
kind = "repo-wide"

[[rule]]
id = "fixture-metadata"
exact = "cargo-metadata.json"
kind = "ignored"

[[rule]]
id = "npm-root"
exact = "package.json"
kind = "repo-wide"

[[rule]]
id = "documentation"
suffix = ".md"
kind = "ignored"
"#;

impl Fixture {
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("a temporary directory"),
            crates: Vec::new(),
            npm: Vec::new(),
            npm_workspaces: vec!["packages/*".to_owned()],
            files: Vec::new(),
            units: "schema = \"aex.units.v1\"\n".to_owned(),
            scenarios: "schema = \"aex.scenario-ownership.v1\"\n".to_owned(),
            path_map: FIXTURE_PATH_MAP.to_owned(),
        }
    }

    #[must_use]
    pub fn add_crate(mut self, plan: CratePlan) -> Self {
        self.files.push(format!("{}/src/lib.rs", plan.dir));
        self.crates.push(plan);
        self
    }

    /// Add an npm member. A root `package.json` is written whenever one is
    /// present, so a fixture without npm members has no npm workspace at all.
    #[must_use]
    pub fn add_npm(mut self, plan: NpmPlan) -> Self {
        self.files.push(format!("{}/src/index.ts", plan.dir));
        self.npm.push(plan);
        self
    }

    #[must_use]
    pub fn file(mut self, path: &str) -> Self {
        self.files.push(path.to_owned());
        self
    }

    #[must_use]
    pub fn units(mut self, toml: &str) -> Self {
        toml.clone_into(&mut self.units);
        self
    }

    #[must_use]
    pub fn scenarios(mut self, toml: &str) -> Self {
        toml.clone_into(&mut self.scenarios);
        self
    }

    #[must_use]
    pub fn path_map(mut self, toml: &str) -> Self {
        toml.clone_into(&mut self.path_map);
        self
    }

    /// Materialize the fixture and return its root.
    pub fn build(self) -> PathBuf {
        let root = self.root.path().to_path_buf();
        // The TempDir guard is leaked deliberately: a fixture outlives the
        // borrow in every test that uses it, and the operating system reclaims
        // the directory. Dropping it here would delete the tree under test.
        std::mem::forget(self.root);

        let packages: Vec<Value> = self
            .crates
            .iter()
            .map(|plan| {
                let mut package = json!({
                    "name": plan.name,
                    "manifest_path": root
                        .join(&plan.dir)
                        .join("Cargo.toml")
                        .to_string_lossy()
                        .replace('\\', "/"),
                    "dependencies": plan
                        .deps
                        .iter()
                        .map(|(name, kind)| json!({ "name": name, "kind": kind }))
                        .collect::<Vec<_>>(),
                });
                if let Some(meta) = &plan.meta {
                    package["metadata"] = json!({ "aex": meta });
                }
                if let Some(publish) = &plan.publish {
                    package["publish"] = publish.clone();
                }
                package
            })
            .collect();

        write(
            &root,
            "cargo-metadata.json",
            &serde_json::to_string_pretty(&json!({ "packages": packages })).unwrap(),
        );
        if !self.npm.is_empty() {
            write(
                &root,
                "package.json",
                &serde_json::to_string_pretty(&json!({
                    "name": "@fixture/repository",
                    "private": true,
                    "workspaces": self.npm_workspaces,
                }))
                .unwrap(),
            );
            for plan in &self.npm {
                let mut manifest = json!({ "name": plan.name, "private": true });
                if let Some(meta) = &plan.meta {
                    manifest["aex"] = meta.clone();
                }
                write(
                    &root,
                    &format!("{}/package.json", plan.dir),
                    &serde_json::to_string_pretty(&manifest).unwrap(),
                );
            }
        }
        write(&root, "release/units.toml", &self.units);
        write(&root, "release/scenario-ownership.toml", &self.scenarios);
        write(&root, "release/path-map.toml", &self.path_map);
        for file in &self.files {
            write(&root, file, "// fixture\n");
        }
        root
    }
}

pub fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("a fixture directory");
    }
    std::fs::write(path, content).expect("a fixture file");
}

/// A three-crate chain: `leaf` <- `middle` <- `top`, plus one deployable with a
/// live companion and a scenario owner. Verifies clean.
pub fn sound_fixture() -> PathBuf {
    Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf"))
        .add_crate(CratePlan::new("aex-middle", "crates/aex-middle").dep("aex-leaf"))
        .add_crate(CratePlan::new("aex-top", "crates/aex-top").dep("aex-middle"))
        .add_crate(
            CratePlan::new("demo-api", "services/demo-api")
                .dep("aex-middle")
                .meta(deployable_meta("demo-api", "aex-live-demo-api")),
        )
        .add_crate(
            CratePlan::new("aex-live-demo-api", "tests/live/aex-live-demo-api")
                .dev_dep("demo-api")
                .meta(live_meta("demo-api")),
        )
        .units(SOUND_UNITS)
        .scenarios(SOUND_SCENARIOS)
        .build()
}

pub const SOUND_UNITS: &str = r#"
schema = "aex.units.v1"

[[unit]]
id = "demo-api"
kind = "rust-lambda"
plane = "regional"
package = "demo-api"
bin = "demo-api"
target = "aarch64-unknown-linux-gnu.2.34"
profile = "release-lambda"
form = "zip"
entrypoint = "bootstrap"
config_env_namespace = "AEX_DEMO_"
config_schema_version = 1
health_path = "/internal/healthz"
ready_path = "/internal/readyz"
required_receipts = ["unit", "lint"]
alarm_spec = "demo-api"
live_suite = "aex-live-demo-api"

[unit.lambda]
memory_mb = 512
timeout_s = 30
reserved_concurrency = 10
"#;

pub const SOUND_SCENARIOS: &str = r#"
schema = "aex.scenario-ownership.v1"

[[scenario]]
id = "SC-DEMO"
owner = "delivery"
observes = ["artifact:demo-api"]
package = "cargo:aex-live-demo-api"
target = "smoke"
"#;
