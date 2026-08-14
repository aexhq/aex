//! The subset of `cargo metadata --no-deps --format-version 1` this tool reads.
//!
//! Dependency edges come from the native manifest, never from a second
//! hand-written graph (`M12`). Only the fields the rules need are modelled, so a
//! unit-test fixture stays small enough to read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Why `cargo metadata` output could not be used.
#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    /// The document is not the expected `cargo metadata` shape.
    #[error("cargo metadata output is not parseable: {0}")]
    Parse(#[from] serde_json::Error),
    /// A member manifest is not inside the workspace root.
    #[error("member `{name}` manifest `{manifest}` is not inside the workspace root")]
    OutsideWorkspace {
        /// The offending package.
        name: String,
        /// Its manifest path.
        manifest: String,
    },
}

/// How a dependency edge is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DependencyKind {
    /// Linked into the package's own targets.
    Normal,
    /// Used only by the package's tests, examples and benches.
    Development,
    /// Used only by the package's build script.
    Build,
}

/// One dependency edge as `cargo metadata` reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct DependencyMeta {
    /// Dependency package name.
    pub name: String,
    /// `null` for a normal dependency, otherwise `dev` or `build`.
    #[serde(default)]
    pub kind: Option<String>,
}

impl DependencyMeta {
    /// How this edge is used.
    #[must_use]
    pub fn kind(&self) -> DependencyKind {
        match self.kind.as_deref() {
            Some("dev") => DependencyKind::Development,
            Some("build") => DependencyKind::Build,
            _ => DependencyKind::Normal,
        }
    }
}

/// One build target as `cargo metadata` reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct TargetMeta {
    /// Target name.
    pub name: String,
    /// Target kinds, such as `lib`, `bin`, `test` or `example`.
    #[serde(default)]
    pub kind: Vec<String>,
    /// Features Cargo requires before this target exists in the selected graph.
    #[serde(default, rename = "required-features")]
    pub required_features: Vec<String>,
}

/// One package as `cargo metadata` reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct PackageMeta {
    /// Opaque package identifier used by `workspace_members`.
    pub id: String,
    /// Package name.
    pub name: String,
    /// Absolute path of the package's `Cargo.toml`.
    pub manifest_path: String,
    /// `[package] description`, which [`crate::description`] projects from the
    /// entry file's `//!` header rather than letting it be written by hand.
    #[serde(default)]
    pub description: Option<String>,
    /// `None` when publishable, `Some(registries)` otherwise; `publish = false`
    /// renders as an empty list.
    #[serde(default)]
    pub publish: Option<Vec<String>>,
    /// Declared build targets.
    #[serde(default)]
    pub targets: Vec<TargetMeta>,
    /// Declared dependency edges.
    #[serde(default)]
    pub dependencies: Vec<DependencyMeta>,
    /// The `[package.metadata]` table, verbatim. `[package.metadata.aex]` lives
    /// inside it and is parsed by [`crate::testmeta`].
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Declared Cargo features and what each enables.
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
}

impl PackageMeta {
    /// Whether the package declares `publish = false`.
    #[must_use]
    pub fn is_unpublished(&self) -> bool {
        self.publish.as_ref().is_some_and(Vec::is_empty)
    }

    /// The package's library target, if it has one.
    #[must_use]
    pub fn library(&self) -> Option<&TargetMeta> {
        self.targets
            .iter()
            .find(|target| target.kind.iter().any(|kind| kind == "lib"))
    }

    /// Whether the package produces an executable.
    #[must_use]
    pub fn has_binary(&self) -> bool {
        self.targets
            .iter()
            .any(|target| target.kind.iter().any(|kind| kind == "bin"))
    }

    /// The names of the package's integration-test and bench targets.
    ///
    /// The library's own `#[cfg(test)]` module is deliberately absent: it is
    /// always layer `unit` and needs no `[package.metadata.aex.targets]` row.
    #[must_use]
    pub fn test_target_names(&self) -> Vec<&str> {
        self.targets
            .iter()
            .filter(|target| {
                target
                    .kind
                    .iter()
                    .any(|kind| kind == "test" || kind == "bench")
            })
            .map(|target| target.name.as_str())
            .collect()
    }

    /// The `[package.metadata.aex]` table, if the manifest declares one.
    #[must_use]
    pub fn aex_metadata(&self) -> Option<&serde_json::Value> {
        self.metadata.as_ref()?.get("aex")
    }
}

/// The whole document.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceMetadata {
    /// Every package in the graph. With `--no-deps` this is exactly the members.
    pub packages: Vec<PackageMeta>,
    /// Package identifiers of the workspace members.
    pub workspace_members: Vec<String>,
    /// Absolute path of the workspace root.
    pub workspace_root: String,
}

impl WorkspaceMetadata {
    /// Parses `cargo metadata --no-deps --format-version 1` output.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataError::Parse`] when the document does not have the
    /// expected shape.
    pub fn parse(json: &str) -> Result<Self, MetadataError> {
        Ok(serde_json::from_str(json)?)
    }

    /// The member packages, in declaration order.
    #[must_use]
    pub fn members(&self) -> Vec<&PackageMeta> {
        self.packages
            .iter()
            .filter(|package| self.workspace_members.contains(&package.id))
            .collect()
    }

    /// Member package name to its directory path relative to the workspace root,
    /// using forward slashes.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataError::OutsideWorkspace`] when a member manifest does
    /// not live under the workspace root.
    pub fn member_directories(&self) -> Result<BTreeMap<String, String>, MetadataError> {
        let root = PathBuf::from(&self.workspace_root);
        let mut directories = BTreeMap::new();
        for package in self.members() {
            let manifest = PathBuf::from(&package.manifest_path);
            let directory = manifest.parent().unwrap_or(Path::new(""));
            let relative =
                directory
                    .strip_prefix(&root)
                    .map_err(|_| MetadataError::OutsideWorkspace {
                        name: package.name.clone(),
                        manifest: package.manifest_path.clone(),
                    })?;
            directories.insert(package.name.clone(), normalise(relative));
        }
        Ok(directories)
    }
}

/// Renders a relative path with forward slashes so rules read the same on every
/// platform.
#[must_use]
pub fn normalise(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::{DependencyKind, WorkspaceMetadata};

    const FIXTURE: &str = r#"{
      "workspace_root": "/w",
      "workspace_members": ["pkg-a 0.1.0 (path+file:///w/crates/aex-a)"],
      "packages": [
        {
          "id": "pkg-a 0.1.0 (path+file:///w/crates/aex-a)",
          "name": "aex-a",
          "manifest_path": "/w/crates/aex-a/Cargo.toml",
          "publish": [],
          "targets": [{ "name": "aex_a", "kind": ["lib"] }],
          "dependencies": [
            { "name": "serde", "kind": null },
            { "name": "aex-regional-test-support", "kind": "dev" },
            { "name": "prost-build", "kind": "build" }
          ]
        },
        {
          "id": "pkg-b 0.1.0 (path+file:///elsewhere)",
          "name": "outsider",
          "manifest_path": "/elsewhere/Cargo.toml",
          "publish": null,
          "targets": [],
          "dependencies": []
        }
      ]
    }"#;

    #[test]
    fn only_workspace_members_are_returned() {
        let metadata = WorkspaceMetadata::parse(FIXTURE).expect("the fixture parses");
        let members = metadata.members();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].name, "aex-a");
    }

    #[test]
    fn dependency_kinds_are_classified() {
        let metadata = WorkspaceMetadata::parse(FIXTURE).expect("the fixture parses");
        let kinds: Vec<DependencyKind> = metadata.members()[0]
            .dependencies
            .iter()
            .map(super::DependencyMeta::kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                DependencyKind::Normal,
                DependencyKind::Development,
                DependencyKind::Build
            ]
        );
    }

    #[test]
    fn publish_false_is_recognised() {
        let metadata = WorkspaceMetadata::parse(FIXTURE).expect("the fixture parses");
        assert!(metadata.members()[0].is_unpublished());
        let publishable = metadata
            .packages
            .iter()
            .find(|package| package.name == "outsider")
            .expect("present");
        assert!(!publishable.is_unpublished());
    }

    #[test]
    fn member_directories_are_relative_and_slash_separated() {
        let metadata = WorkspaceMetadata::parse(FIXTURE).expect("the fixture parses");
        let directories = metadata
            .member_directories()
            .expect("members live under the root");
        assert_eq!(
            directories.get("aex-a").map(String::as_str),
            Some("crates/aex-a")
        );
    }

    #[test]
    fn the_library_target_is_found() {
        let metadata = WorkspaceMetadata::parse(FIXTURE).expect("the fixture parses");
        let package = metadata.members()[0];
        assert_eq!(
            package.library().map(|target| target.name.as_str()),
            Some("aex_a")
        );
        assert!(!package.has_binary());
    }
}
