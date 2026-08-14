//! Immutable Hands image selection.
//!
//! A release publishes one general-purpose 2 GiB image. The catalog is resolved
//! once from the authenticated release placement, validated as a closed one-row
//! document at process start, and then selected only by the
//! [`ImagePin`], so generation allocation can persist the exact provider ARN,
//! provider version and artifact digest before any launch is attempted.

use std::collections::{BTreeMap, BTreeSet};

use aex_wire::ids::ContentHash;
use aex_wire::types::ComputeSize;
use serde::{Deserialize, Serialize};

use crate::generation::{ImageCapability, ImageIdentifier, ImagePin, ImageVersion};
use crate::shape::ShapeCapacity as _;

/// Package managers installed in every published Hands image variant.
pub const HANDS_PACKAGE_ECOSYSTEMS: [aex_wire::models::PackageEcosystem; 3] = [
    aex_wire::models::PackageEcosystem::Apt,
    aex_wire::models::PackageEcosystem::Npm,
    aex_wire::models::PackageEcosystem::Pip,
];

/// The published release variant keys, in deterministic selection order.
///
/// This must equal the `hands-image` unit's MicroVM variant in
/// `release/units.toml`.
pub const HANDS_IMAGE_VARIANTS: [&str; 1] = ["2gb"];

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
    #[error("Hands image catalog keys must be exactly the published 2gb release variant")]
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
    /// The requested public shape does not offer all required capabilities.
    #[error("compute size `{size}` has no browser-capable Hands image")]
    CapabilityUnavailable {
        /// Public shape that cannot carry the capability.
        size: &'static str,
    },
    /// The requested legacy size is not part of the launch catalog.
    #[error("compute size `{size}` is unavailable; the launch image is 2gb")]
    SizeUnavailable {
        /// Requested public token.
        size: &'static str,
    },
}

/// Why release-derived image deployment facts could not become selection authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeControlReleaseCatalogError {
    /// The release fact was not the closed JSON catalog.
    #[error("expected the closed release JSON catalog: {0}")]
    Decode(String),
    /// The decoded catalog violated the exact published variant contract.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// An image belongs to another plane, account, or region.
    #[error(
        "image ARN `{identifier}` is outside plane `{plane}`, account `{account}` or region `{region}`"
    )]
    ForeignBinding {
        /// Provider image identity.
        identifier: String,
        /// Expected plane.
        plane: String,
        /// Expected account.
        account: String,
        /// Expected region.
        region: String,
    },
    /// An image ARN was not named by its immutable base32 content identity.
    #[error("image ARN `{0}` is not content-addressed")]
    NotContentAddressed(String),
}

impl HandsImageCatalog {
    /// Parses the exact release-derived catalog and binds every image ARN to
    /// the process plane, account, and region.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeControlReleaseCatalogError`] for malformed/partial catalogs, foreign
    /// provider identities, or mutable/non-content-addressed image names.
    pub fn from_release_json(
        raw: &str,
        plane: &str,
        region: &str,
        account: &str,
    ) -> Result<Self, RuntimeControlReleaseCatalogError> {
        let entries = serde_json::from_str::<BTreeMap<String, HandsImageCatalogEntry>>(raw)
            .map_err(|error| RuntimeControlReleaseCatalogError::Decode(error.to_string()))?;
        let catalog = Self::from_entries(entries)?;
        let prefix = format!("arn:aws:lambda:{region}:{account}:microvm-image:aex-{plane}-");
        for identifier in catalog.image_identifiers() {
            let Some(suffix) = identifier.0.strip_prefix(&prefix) else {
                return Err(RuntimeControlReleaseCatalogError::ForeignBinding {
                    identifier: identifier.0.clone(),
                    plane: plane.to_owned(),
                    account: account.to_owned(),
                    region: region.to_owned(),
                });
            };
            if suffix.len() != 52
                || !suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
            {
                return Err(RuntimeControlReleaseCatalogError::NotContentAddressed(
                    identifier.0.clone(),
                ));
            }
        }
        Ok(catalog)
    }

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
        }
        Ok(Self { entries })
    }

    /// Selects the exact immutable image for one generation definition.
    ///
    /// The selected pair is cloned into the immutable generation record; launch
    /// retries never consult this catalog again.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError::CapabilityUnavailable`] when browser is requested,
    /// or [`CatalogError::SizeUnavailable`] for a retired launch size.
    pub fn select(
        &self,
        size: ComputeSize,
        required_capabilities: &[ImageCapability],
    ) -> Result<ImagePin, CatalogError> {
        if required_capabilities.contains(&ImageCapability::Browser) {
            return Err(CatalogError::CapabilityUnavailable {
                size: size.as_str(),
            });
        }
        if size != ComputeSize::Gb2 {
            return Err(CatalogError::SizeUnavailable {
                size: size.as_str(),
            });
        }
        let entry = self.entries.get("2gb").ok_or(CatalogError::VariantSet)?;
        Ok(ImagePin {
            identifier: entry.image_arn.clone(),
            version: entry.image_version.clone(),
            artifact_digest: entry.artifact_digest,
            capabilities: Vec::new(),
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
        "2gb" => (ComputeSize::Gb2, false),
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
                (
                    variant.to_owned(),
                    HandsImageCatalogEntry {
                        image_arn: ImageIdentifier(format!("arn:image:{index}")),
                        image_version: ImageVersion((index + 1).to_string()),
                        artifact_digest: ContentHash::from_bytes([digest_byte; 32]),
                        minimum_memory_mib: 2_048,
                        browser: false,
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
        missing.remove("2gb");
        assert_eq!(
            HandsImageCatalog::from_entries(missing),
            Err(CatalogError::VariantSet)
        );

        // A browser variant is not published, so an extra row is not accepted.
        let mut extra = complete.clone();
        extra.insert("2gb-browser".to_owned(), complete["2gb"].clone());
        assert_eq!(
            HandsImageCatalog::from_entries(extra),
            Err(CatalogError::VariantSet)
        );

        let mut wrong_shape = complete;
        wrong_shape.get_mut("2gb").expect("row").minimum_memory_mib = 4_096;
        assert!(matches!(
            HandsImageCatalog::from_entries(wrong_shape),
            Err(CatalogError::Shape { variant }) if variant == "2gb"
        ));
    }

    #[test]
    fn capability_and_memory_select_the_exact_pin() {
        let catalog = HandsImageCatalog::from_entries(entries()).expect("catalog");
        let base = catalog.select(ComputeSize::Gb2, &[]).expect("base image");
        assert_eq!(base.identifier.0, "arn:image:0");
        assert_eq!(base.version.0, "1");
        assert!(base.capabilities.is_empty());
    }

    #[test]
    fn the_launch_catalog_refuses_browser_and_retired_sizes() {
        let catalog = HandsImageCatalog::from_entries(entries()).expect("catalog");
        assert_eq!(
            catalog.select(ComputeSize::Gb2, &[ImageCapability::Browser]),
            Err(CatalogError::CapabilityUnavailable { size: "2gb" })
        );
        for size in [
            ComputeSize::Mb512,
            ComputeSize::Gb1,
            ComputeSize::Gb4,
            ComputeSize::Gb8,
        ] {
            assert!(matches!(
                catalog.select(size, &[]),
                Err(CatalogError::SizeUnavailable { .. })
            ));
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
