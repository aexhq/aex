//! Deterministic `CycloneDX` projection of the shipped guest dependency closure.
//!
//! The image recipe needs the inventory before it can write its service ZIP.
//! Generating it from Cargo's resolved graph here avoids an unrecorded workflow
//! step and excludes development-only crates that cannot reach the guest binary.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, Package, PackageId};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Bom {
    #[serde(rename = "bomFormat")]
    format: &'static str,
    spec_version: &'static str,
    version: u32,
    metadata: BomMetadata,
    components: Vec<Component>,
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Serialize)]
struct BomMetadata {
    component: Component,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Component {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "bom-ref")]
    bom_ref: String,
    name: String,
    version: String,
    purl: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Dependency {
    #[serde(rename = "ref")]
    reference: String,
    depends_on: Vec<String>,
}

fn package_ref(package: &Package) -> String {
    format!("pkg:cargo/{}@{}", package.name, package.version)
}

fn component(package: &Package, kind: &'static str) -> Component {
    let purl = package_ref(package);
    Component {
        kind,
        bom_ref: purl.clone(),
        name: package.name.to_string(),
        version: package.version.to_string(),
        purl,
    }
}

fn shipped_edges(metadata: &Metadata, root: &PackageId) -> BTreeMap<String, BTreeSet<String>> {
    let Some(resolve) = metadata.resolve.as_ref() else {
        return BTreeMap::new();
    };
    let packages: BTreeMap<String, &Package> = metadata
        .packages
        .iter()
        .map(|package| (package.id.repr.clone(), package))
        .collect();
    let nodes: BTreeMap<String, _> = resolve
        .nodes
        .iter()
        .map(|node| (node.id.repr.clone(), node))
        .collect();
    let mut graph = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut frontier = vec![root.repr.clone()];
    while let Some(id) = frontier.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(package) = packages.get(&id) else {
            continue;
        };
        let Some(node) = nodes.get(&id) else {
            continue;
        };
        let mut dependencies = BTreeSet::new();
        for dependency in &node.deps {
            let shipped = dependency
                .dep_kinds
                .iter()
                .any(|kind| matches!(kind.kind, DependencyKind::Normal | DependencyKind::Build));
            if !shipped {
                continue;
            }
            let Some(child) = packages.get(&dependency.pkg.repr) else {
                continue;
            };
            dependencies.insert(package_ref(child));
            frontier.push(dependency.pkg.repr.clone());
        }
        graph.insert(package_ref(package), dependencies);
    }
    graph
}

/// Generate stable `CycloneDX` JSON for the target-filtered `hands-agent` closure.
///
/// # Errors
/// Returns a typed error when Cargo metadata fails, the package/resolve graph is
/// absent, or the resulting document cannot be encoded.
pub fn generate(manifest: &Path, target: &str) -> Result<Vec<u8>, SbomError> {
    let mut command = MetadataCommand::new();
    command.manifest_path(manifest);
    command.other_options(vec![
        "--locked".to_owned(),
        "--filter-platform".to_owned(),
        target.to_owned(),
    ]);
    let metadata = command.exec().map_err(SbomError::Metadata)?;
    let root = metadata
        .packages
        .iter()
        .find(|package| package.name.as_str() == "hands-agent")
        .ok_or(SbomError::RootMissing)?;
    let edges = shipped_edges(&metadata, &root.id);
    if edges.is_empty() {
        return Err(SbomError::ResolveMissing);
    }
    let packages: BTreeMap<String, &Package> = metadata
        .packages
        .iter()
        .map(|package| (package_ref(package), package))
        .collect();
    let components = edges
        .keys()
        .filter(|candidate| *candidate != &package_ref(root))
        .filter_map(|reference| packages.get(reference))
        .map(|package| component(package, "library"))
        .collect();
    let dependencies = edges
        .into_iter()
        .map(|(reference, children)| Dependency {
            reference,
            depends_on: children.into_iter().collect(),
        })
        .collect();
    let bom = Bom {
        format: "CycloneDX",
        spec_version: "1.5",
        version: 1,
        metadata: BomMetadata {
            component: component(root, "application"),
        },
        components,
        dependencies,
    };
    let mut encoded = serde_json::to_vec(&bom).map_err(SbomError::Encode)?;
    encoded.push(b'\n');
    Ok(encoded)
}

/// Why the dependency inventory could not be generated.
#[derive(Debug, thiserror::Error)]
pub enum SbomError {
    /// Cargo could not resolve the locked, target-filtered graph.
    #[error("cargo metadata failed: {0}")]
    Metadata(cargo_metadata::Error),
    /// The workspace did not contain the guest package.
    #[error("the Cargo graph has no hands-agent package")]
    RootMissing,
    /// Cargo returned no traversable resolve graph.
    #[error("the Cargo graph has no shipped hands-agent closure")]
    ResolveMissing,
    /// The deterministic document could not be encoded.
    #[error("CycloneDX JSON encoding failed: {0}")]
    Encode(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::generate;

    #[test]
    fn the_guest_sbom_is_stable_and_excludes_development_only_dependencies() {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hands-agent/Cargo.toml");
        let first = generate(&manifest, crate::image::GUEST_TARGET).expect("first inventory");
        let second = generate(&manifest, crate::image::GUEST_TARGET).expect("second inventory");
        assert_eq!(first, second);
        let document: serde_json::Value = serde_json::from_slice(&first).expect("valid JSON");
        assert_eq!(document["bomFormat"], "CycloneDX");
        assert_eq!(document["specVersion"], "1.5");
        assert_eq!(document["metadata"]["component"]["name"], "hands-agent");
        let encoded = String::from_utf8(first).expect("UTF-8 JSON");
        assert!(encoded.contains("aex-hands-agent"));
        assert!(
            !encoded.contains("cargo_metadata"),
            "the package is a test-only dependency of hands-agent"
        );
    }
}
