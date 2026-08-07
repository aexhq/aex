//! Immutable Hands image selection.
//!
//! A release publishes the five base images.  The three browser-capable variants
//! are excluded during prelaunch -- see the browser-variant entry in
//! `references/backlog.md` -- because the pinned AL2023 ARM64 repository
//! publishes no Chromium,
//! so listing them would mint artifacts that deterministically fail in the
//! provider image builder.  The catalog is resolved once from the authenticated
//! release placement, validated as a closed five-row document at process start,
//! and then selected only by the
//! [`ImagePin`], so generation allocation can persist the exact provider ARN,
//! provider version and artifact digest before any launch is attempted.

use std::collections::{BTreeMap, BTreeSet};

use aex_wire::ids::ContentHash;
use aex_wire::types::ComputeSize;
use serde::{Deserialize, Serialize};

use crate::generation::{ImageCapability, ImageIdentifier, ImagePin, ImageVersion};
use crate::shape::ShapeCapacity as _;

/// The published release variant keys, in deterministic selection order.
///
/// This must equal the `hands-image-*` unit set in `release/units.toml`. It
/// listed the three browser variants while the release published five, so
/// `runtime-control-worker` refused every start and the runtime reaper never
/// swept. Restoring a browser row here requires restoring its release unit in
/// the same change.
pub const HANDS_IMAGE_VARIANTS: [&str; 5] = ["512mb", "1gb", "2gb", "4gb", "8gb"];

/// One immutable resolved image identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HandsImageCatalogEntry {
    /// Exact provider image ARN.
    pub image_arn: ImageIdentifier,
    /// Exact immutable provider version under that ARN.
    pub image_version: ImageVersion,
    /// Digest of the public release artifact used to create the image.
    pub artifact_digest: ContentHash,
    /// Provider `minimumMemoryInMiB`, which must equal the public shape.
    #[serde(rename = "minimumMemoryMiB")]
    pub minimum_memory_mib: u32,
    /// Whether the image contains the browser capability layer.
    pub browser: bool,
}

/// A validated closed catalog. Invalid or partial documents cannot be selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandsImageCatalog {
    entries: BTreeMap<String, HandsImageCatalogEntry>,
}

/// Why a release catalog could not become generation-allocation authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// The document did not contain exactly the published release variants.
    #[error("Hands image catalog keys must be exactly the five published release variants")]
    VariantSet,
    /// An entry did not agree with the size encoded by its key.
    #[error(
        "Hands image catalog variant `{variant}` has an invalid minimum memory or browser flag"
    )]
    Shape {
        /// Variant carrying the inconsistent shape.
        variant: String,
    },
    /// The provider identity is absent.
    #[error("Hands image catalog variant `{variant}` has an empty image ARN or version")]
    EmptyIdentity {
        /// Variant with the absent identity component.
        variant: String,
    },
    /// Two variants point at one provider image ARN.
    #[error("Hands image catalog image ARN `{identifier}` is assigned to more than one variant")]
    DuplicateImage {
        /// Reused provider identifier.
        identifier: String,
    },
    /// The requested public shape does not offer all required capabilities.
    #[error("compute size `{size}` has no browser-capable Hands image")]
    CapabilityUnavailable {
        /// Public shape that cannot carry the capability.
        size: &'static str,
    },
}

impl HandsImageCatalog {
    /// Validates a decoded environment document as the exact release catalog.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] for a missing/extra variant, a shape mismatch,
    /// an absent identity, or a reused provider ARN.
    pub fn from_entries(
        entries: BTreeMap<String, HandsImageCatalogEntry>,
    ) -> Result<Self, CatalogError> {
        let actual = entries.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let expected = HANDS_IMAGE_VARIANTS.into_iter().collect::<BTreeSet<_>>();
        if actual != expected {
            return Err(CatalogError::VariantSet);
        }

        let mut identifiers = BTreeSet::new();
        for (variant, entry) in &entries {
            let (size, browser) = variant_shape(variant).ok_or(CatalogError::VariantSet)?;
            if entry.minimum_memory_mib != size.minimum_memory_mib() || entry.browser != browser {
                return Err(CatalogError::Shape {
                    variant: variant.clone(),
                });
            }
            if entry.image_arn.0.is_empty() || entry.image_version.0.is_empty() {
                return Err(CatalogError::EmptyIdentity {
                    variant: variant.clone(),
                });
            }
            if !identifiers.insert(entry.image_arn.0.clone()) {
                return Err(CatalogError::DuplicateImage {
                    identifier: entry.image_arn.0.clone(),
                });
            }
        }
        Ok(Self { entries })
    }

    /// Selects the exact immutable image for one generation definition.
    ///
    /// Browser is the only optional image capability in strict v1.  The selected
    /// pair is cloned into the immutable generation record; launch retries never
    /// consult this catalog again.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError::CapabilityUnavailable`] when browser is requested
    /// for a shape below 2 GiB, or for any shape while no browser variant is
    /// published.
    pub fn select(
        &self,
        size: ComputeSize,
        required_capabilities: &[ImageCapability],
    ) -> Result<ImagePin, CatalogError> {
        let browser = required_capabilities.contains(&ImageCapability::Browser);
        if browser && !size.offers_browser() {
            return Err(CatalogError::CapabilityUnavailable {
                size: size.as_str(),
            });
        }
        let key = if browser {
            format!("{}-browser", size.as_str())
        } else {
            size.as_str().to_owned()
        };
        // A missing browser row is an unavailable capability, not a malformed
        // document: `from_entries` already proved the catalog holds exactly the
        // published variants, so the only way to miss here is to ask for one that
        // this release does not publish. `VariantSet` would send a reader hunting
        // for corruption that is not there.
        let entry = self.entries.get(&key).ok_or(if browser {
            CatalogError::CapabilityUnavailable {
                size: size.as_str(),
            }
        } else {
            CatalogError::VariantSet
        })?;
        Ok(ImagePin {
            identifier: entry.image_arn.clone(),
            version: entry.image_version.clone(),
            artifact_digest: entry.artifact_digest,
            capabilities: if browser {
                vec![ImageCapability::Browser]
            } else {
                Vec::new()
            },
        })
    }

    /// Every exact image identifier, for the bounded startup provider probe.
    #[must_use]
    pub fn image_identifiers(&self) -> impl ExactSizeIterator<Item = &ImageIdentifier> {
        self.entries.values().map(|entry| &entry.image_arn)
    }
}

fn variant_shape(variant: &str) -> Option<(ComputeSize, bool)> {
    Some(match variant {
        "512mb" => (ComputeSize::Mb512, false),
        "1gb" => (ComputeSize::Gb1, false),
        "2gb" => (ComputeSize::Gb2, false),
        "2gb-browser" => (ComputeSize::Gb2, true),
        "4gb" => (ComputeSize::Gb4, false),
        "4gb-browser" => (ComputeSize::Gb4, true),
        "8gb" => (ComputeSize::Gb8, false),
        "8gb-browser" => (ComputeSize::Gb8, true),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{CatalogError, HANDS_IMAGE_VARIANTS, HandsImageCatalog, HandsImageCatalogEntry};
    use crate::generation::{ImageCapability, ImageIdentifier, ImageVersion};
    use aex_wire::ids::ContentHash;
    use aex_wire::types::ComputeSize;
    use std::collections::BTreeMap;

    fn entries() -> BTreeMap<String, HandsImageCatalogEntry> {
        HANDS_IMAGE_VARIANTS
            .into_iter()
            .enumerate()
            .map(|(index, variant)| {
                let digest_byte = u8::try_from(index).expect("the catalog is small");
                let size = match variant.split('-').next().expect("a size") {
                    "512mb" => 512,
                    "1gb" => 1_024,
                    "2gb" => 2_048,
                    "4gb" => 4_096,
                    "8gb" => 8_192,
                    _ => unreachable!(),
                };
                (
                    variant.to_owned(),
                    HandsImageCatalogEntry {
                        image_arn: ImageIdentifier(format!("arn:image:{index}")),
                        image_version: ImageVersion((index + 1).to_string()),
                        artifact_digest: ContentHash::from_bytes([digest_byte; 32]),
                        minimum_memory_mib: size,
                        browser: variant.ends_with("-browser"),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn exactly_the_published_shape_consistent_unique_rows_are_required() {
        let complete = entries();
        assert!(HandsImageCatalog::from_entries(complete.clone()).is_ok());

        let mut missing = complete.clone();
        missing.remove("8gb");
        assert_eq!(
            HandsImageCatalog::from_entries(missing),
            Err(CatalogError::VariantSet)
        );

        // A browser variant is not published, so an entry claiming the capability
        // is a shape violation rather than an unknown key.
        let mut extra = complete.clone();
        extra.insert("8gb-browser".to_owned(), complete["8gb"].clone());
        assert_eq!(
            HandsImageCatalog::from_entries(extra),
            Err(CatalogError::VariantSet)
        );

        let mut wrong_shape = complete.clone();
        wrong_shape.get_mut("4gb").expect("row").browser = true;
        assert!(matches!(
            HandsImageCatalog::from_entries(wrong_shape),
            Err(CatalogError::Shape { variant }) if variant == "4gb"
        ));

        let mut duplicate = complete;
        duplicate.get_mut("8gb").expect("row").image_arn = duplicate["4gb"].image_arn.clone();
        assert!(matches!(
            HandsImageCatalog::from_entries(duplicate),
            Err(CatalogError::DuplicateImage { .. })
        ));
    }

    #[test]
    fn capability_and_memory_select_the_exact_pin() {
        let catalog = HandsImageCatalog::from_entries(entries()).expect("catalog");
        let base = catalog.select(ComputeSize::Gb4, &[]).expect("base image");
        assert_eq!(base.identifier.0, "arn:image:3");
        assert_eq!(base.version.0, "4");
        assert!(base.capabilities.is_empty());
    }

    #[test]
    fn the_browser_capability_is_unavailable_at_every_published_size() {
        // No browser variant is published during prelaunch, so asking for the
        // capability must name the size and refuse rather than silently hand back
        // a base image that cannot do it.
        let catalog = HandsImageCatalog::from_entries(entries()).expect("catalog");
        for (size, name) in [
            (ComputeSize::Gb1, "1gb"),
            (ComputeSize::Gb2, "2gb"),
            (ComputeSize::Gb4, "4gb"),
            (ComputeSize::Gb8, "8gb"),
        ] {
            assert_eq!(
                catalog.select(size, &[ImageCapability::Browser]),
                Err(CatalogError::CapabilityUnavailable { size: name })
            );
        }
    }

    #[test]
    fn catalog_json_is_closed_and_uses_release_identity_names() {
        let json = serde_json::to_string(&entries()).expect("catalog serializes");
        assert!(json.contains("imageArn"));
        assert!(json.contains("imageVersion"));
        assert!(json.contains("artifactDigest"));
        assert!(json.contains("minimumMemoryMiB"));
        let decoded = serde_json::from_str(&json).expect("catalog decodes");
        assert!(HandsImageCatalog::from_entries(decoded).is_ok());
    }
}
