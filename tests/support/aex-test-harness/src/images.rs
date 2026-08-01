//! The pinned container-image registry.
//!
//! `release/policy/test-images.toml` is the single source of truth and it is
//! embedded here, so an integration target asks for an image by key and gets a
//! digest-pinned reference or a typed error. No image name, tag or digest may
//! appear anywhere else in the workspace - the registry checker's
//! `data-image-literal` rule enforces that, and this module is the reason the
//! rule costs a stream nothing to obey.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// Why an image could not be produced.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    /// The key is not in the policy document.
    #[error("`{key}` is not in release/policy/test-images.toml; permitted keys: {permitted}")]
    Unknown {
        /// The key that was asked for.
        key: String,
        /// The keys that exist, comma separated.
        permitted: String,
    },
    /// The entry exists but carries no digest.
    #[error(
        "image `{key}` ({repository}:{tag}) has no digest in release/policy/test-images.toml; every testcontainers image is pinned by digest, not tag"
    )]
    Unpinned {
        /// The key that is unpinned.
        key: String,
        /// Its repository.
        repository: String,
        /// Its tag, recorded for humans only.
        tag: String,
    },
}

/// One pinned image.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ImageRef {
    /// The registry host.
    pub registry: String,
    /// The repository path.
    pub repository: String,
    /// The tag, recorded for reading and for the pin-refresh command.
    pub tag: String,
    /// The digest that actually runs.
    pub digest: String,
    /// What this image is used for.
    pub purpose: String,
    /// What it proves.
    pub proves: String,
    /// What it cannot prove, which is why its seams require live evidence.
    pub cannot_prove: String,
    /// The seam ids this image partially covers.
    #[serde(default)]
    pub seams: Vec<String>,
}

impl ImageRef {
    /// The digest-pinned reference to hand to `testcontainers`.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::Unpinned`] when the policy entry carries no
    /// digest, so an unpinned image can never silently become "whatever the
    /// registry serves today".
    pub fn reference(&self, key: &str) -> Result<String, ImageError> {
        if self.digest.is_empty() {
            return Err(ImageError::Unpinned {
                key: key.to_owned(),
                repository: self.repository.clone(),
                tag: self.tag.clone(),
            });
        }
        Ok(format!(
            "{}/{}@{}",
            self.registry, self.repository, self.digest
        ))
    }
}

#[derive(Debug, Deserialize)]
struct Document {
    image: BTreeMap<String, ImageRef>,
}

fn table() -> &'static BTreeMap<String, ImageRef> {
    static TABLE: OnceLock<BTreeMap<String, ImageRef>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let document: Document = toml::from_str(crate::TEST_IMAGES_TOML)
            .expect("release/policy/test-images.toml is embedded and must parse");
        document.image
    })
}

/// Every pinned image key, sorted.
#[must_use]
pub fn keys() -> Vec<&'static str> {
    table().keys().map(String::as_str).collect()
}

/// The pinned image for `key`.
///
/// # Errors
///
/// Returns [`ImageError::Unknown`] when the key is not in the policy document.
pub fn image(key: &str) -> Result<&'static ImageRef, ImageError> {
    table().get(key).ok_or_else(|| ImageError::Unknown {
        key: key.to_owned(),
        permitted: keys().join(", "),
    })
}

/// The digest-pinned reference for `key`.
///
/// # Errors
///
/// Returns [`ImageError`] when the key is unknown or the entry is unpinned.
pub fn reference(key: &str) -> Result<String, ImageError> {
    image(key)?.reference(key)
}

#[cfg(test)]
mod tests {
    use super::{ImageError, ImageRef, image, keys, reference};

    #[test]
    fn the_policy_document_holds_exactly_the_pinned_substrate() {
        assert_eq!(
            keys(),
            vec![
                "dynamodb_local",
                "localstack",
                "minio",
                "postgres",
                "stripe_mock",
                "toxiproxy"
            ]
        );
    }

    #[test]
    fn every_image_is_pinned_by_digest_and_resolves_to_a_digest_reference() {
        for key in keys() {
            let entry = image(key).expect("the key is in the document");
            assert!(
                entry.digest.starts_with("sha256:") && entry.digest.len() == 71,
                "`{key}` is not digest-pinned: `{}`",
                entry.digest
            );
            let rendered = reference(key).expect("a pinned image resolves");
            assert!(rendered.contains('@'), "{rendered}");
            assert!(!rendered.contains(&format!(":{}", entry.tag)), "{rendered}");
        }
    }

    #[test]
    fn every_image_states_what_it_cannot_prove() {
        for key in keys() {
            let entry = image(key).expect("the key is in the document");
            assert!(
                !entry.cannot_prove.trim().is_empty(),
                "`{key}` claims to prove everything, which no local substitute does"
            );
        }
    }

    #[test]
    fn an_unknown_key_names_the_permitted_set() {
        let error = image("clickhouse").expect_err("ClickHouse is not in the substrate");
        assert!(matches!(error, ImageError::Unknown { .. }));
        assert!(
            error.to_string().contains("permitted keys: dynamodb_local"),
            "{error}"
        );
    }

    #[test]
    fn an_unpinned_entry_refuses_to_produce_a_reference() {
        let unpinned = ImageRef {
            registry: "registry-1.docker.io".to_owned(),
            repository: "library/postgres".to_owned(),
            tag: "17.5-bookworm".to_owned(),
            digest: String::new(),
            purpose: String::new(),
            proves: String::new(),
            cannot_prove: String::new(),
            seams: Vec::new(),
        };
        let error = unpinned
            .reference("postgres")
            .expect_err("an unpinned image is refused");
        assert_eq!(
            error.to_string(),
            "image `postgres` (library/postgres:17.5-bookworm) has no digest in release/policy/test-images.toml; every testcontainers image is pinned by digest, not tag"
        );
    }
}
