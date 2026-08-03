//! Honest implementations of the Lambda `MicroVM` image build hooks.
//!
//! `/ready` verifies the shared rootfs contract. `/validate` additionally asks
//! RPM for the live installed-package set and compares it with both inventories
//! generated into the image. These checks run in a blocking worker because
//! filesystem traversal and process creation must not stop the guest reactor.

use std::path::PathBuf;

use aex_hands_agent::image_contract::{
    AGENT_SBOM_PATH, FORBIDDEN_ROOTFS_PATHS, IMAGE_LOCK_PATH, ImageLock, LockVerdict,
    ROOTFS_CONTRACT, RPM_LIST_PATH,
};

const MAX_AGENT_SBOM_BYTES: u64 = 16 * 1024 * 1024;

/// A build-hook validator composed into the guest.
pub trait ImageValidator: Send + Sync + core::fmt::Debug {
    /// Verify the rootfs shape before Lambda takes a snapshot.
    fn ready(&self) -> Result<(), ImageError>;

    /// Independently verify the installed package set after the snapshot boots.
    fn validate(&self) -> Result<(), ImageError>;
}

/// Production image validation rooted at the guest filesystem.
#[derive(Debug, Clone)]
pub struct HostImageValidator {
    root: PathBuf,
}

impl HostImageValidator {
    /// Validate the real guest root.
    #[must_use]
    pub fn guest() -> Self {
        Self {
            root: PathBuf::from("/"),
        }
    }

    #[cfg(test)]
    fn rooted_at(root: PathBuf) -> Self {
        Self { root }
    }

    fn resolve(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    fn validate_documents(&self, observed: &[String]) -> Result<(), ImageError> {
        let lock_path = self.resolve(IMAGE_LOCK_PATH);
        let encoded = std::fs::read_to_string(&lock_path).map_err(|source| ImageError::Io {
            path: lock_path.clone(),
            source,
        })?;
        let lock: ImageLock =
            serde_json::from_str(&encoded).map_err(|source| ImageError::Lock {
                path: lock_path,
                reason: source.to_string(),
            })?;
        lock.validate().map_err(|reason| ImageError::Lock {
            path: self.resolve(IMAGE_LOCK_PATH),
            reason: reason.to_owned(),
        })?;

        let listing_path = self.resolve(RPM_LIST_PATH);
        let listing = std::fs::read_to_string(&listing_path).map_err(|source| ImageError::Io {
            path: listing_path.clone(),
            source,
        })?;
        let recorded = canonical_lines(&listing)?;
        if recorded != lock.packages {
            return Err(ImageError::InventoryDisagreement);
        }
        match lock.compare(observed) {
            LockVerdict::Match => Ok(()),
            LockVerdict::Drift {
                missing,
                unexpected,
            } => Err(ImageError::PackageDrift {
                missing: missing.len(),
                unexpected: unexpected.len(),
            }),
        }
    }

    fn validate_agent_sbom(&self) -> Result<(), ImageError> {
        let path = self.resolve(AGENT_SBOM_PATH);
        let metadata = std::fs::metadata(&path).map_err(|source| ImageError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.len() == 0 || metadata.len() > MAX_AGENT_SBOM_BYTES {
            return Err(ImageError::Sbom {
                path,
                reason: "document size is outside the accepted range".to_owned(),
            });
        }
        let encoded = std::fs::read(&path).map_err(|source| ImageError::Io {
            path: path.clone(),
            source,
        })?;
        let document: serde_json::Value =
            serde_json::from_slice(&encoded).map_err(|source| ImageError::Sbom {
                path: path.clone(),
                reason: source.to_string(),
            })?;
        if document
            .get("bomFormat")
            .and_then(serde_json::Value::as_str)
            != Some("CycloneDX")
        {
            return Err(ImageError::Sbom {
                path,
                reason: "bomFormat is not CycloneDX".to_owned(),
            });
        }
        Ok(())
    }
}

impl ImageValidator for HostImageValidator {
    fn ready(&self) -> Result<(), ImageError> {
        for entry in ROOTFS_CONTRACT {
            let path = self.resolve(entry.path);
            let metadata = std::fs::symlink_metadata(&path).map_err(|source| ImageError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink()
                || metadata.is_dir() != entry.directory
                || metadata.is_file() == entry.directory
            {
                return Err(ImageError::WrongKind { path });
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let observed = metadata.permissions().mode() & 0o777;
                if observed != entry.mode {
                    return Err(ImageError::WrongMode {
                        path,
                        expected: entry.mode,
                        observed,
                    });
                }
            }
        }
        for forbidden in FORBIDDEN_ROOTFS_PATHS {
            let path = self.resolve(forbidden);
            match std::fs::symlink_metadata(&path) {
                Ok(_) => return Err(ImageError::ForbiddenPath { path }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => return Err(ImageError::Io { path, source }),
            }
        }
        self.validate_agent_sbom()?;
        Ok(())
    }

    fn validate(&self) -> Result<(), ImageError> {
        self.ready()?;
        let output = std::process::Command::new("rpm")
            .args([
                "-qa",
                "--qf",
                "%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\\n",
            ])
            .output()
            .map_err(ImageError::Rpm)?;
        if !output.status.success() {
            return Err(ImageError::RpmStatus(output.status));
        }
        let stdout = String::from_utf8(output.stdout).map_err(ImageError::RpmUtf8)?;
        let mut observed: Vec<String> = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        if observed.is_empty() {
            return Err(ImageError::NonCanonicalInventory);
        }
        observed.sort_unstable();
        if observed.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ImageError::NonCanonicalInventory);
        }
        self.validate_documents(&observed)
    }
}

fn canonical_lines(encoded: &str) -> Result<Vec<String>, ImageError> {
    let lines: Vec<String> = encoded
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if lines.is_empty()
        || lines.windows(2).any(|pair| pair[0] >= pair[1])
        || encoded != format!("{}\n", lines.join("\n"))
    {
        return Err(ImageError::NonCanonicalInventory);
    }
    Ok(lines)
}

/// Why an image build hook failed closed.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// A required path could not be inspected.
    #[error("image path `{}` is unreadable: {source}", path.display())]
    Io {
        /// The path being checked.
        path: PathBuf,
        /// The I/O failure.
        source: std::io::Error,
    },
    /// A required file or directory has the wrong type or is a symlink.
    #[error("image path `{}` has the wrong file type", path.display())]
    WrongKind {
        /// The rejected path.
        path: PathBuf,
    },
    /// A required path has different POSIX permissions.
    #[cfg(unix)]
    #[error(
        "image path `{}` has mode {observed:o}, expected {expected:o}",
        path.display()
    )]
    WrongMode {
        /// The rejected path.
        path: PathBuf,
        /// Contract mode.
        expected: u32,
        /// Observed mode.
        observed: u32,
    },
    /// A deleted artifact reappeared.
    #[error("forbidden image path `{}` exists", path.display())]
    ForbiddenPath {
        /// The rejected path.
        path: PathBuf,
    },
    /// The image-lock document is invalid.
    #[error("image lock `{}` is invalid: {reason}", path.display())]
    Lock {
        /// The rejected document.
        path: PathBuf,
        /// Stable decode or validation reason.
        reason: String,
    },
    /// The embedded Rust dependency inventory is absent or malformed.
    #[error("agent SBOM `{}` is invalid: {reason}", path.display())]
    Sbom {
        /// The rejected document.
        path: PathBuf,
        /// Stable validation reason.
        reason: String,
    },
    /// The text and JSON inventories disagree before the live RPM sample.
    #[error("the embedded RPM listing and image lock disagree")]
    InventoryDisagreement,
    /// A package inventory is empty, unsorted, or contains duplicates.
    #[error("an RPM inventory is empty or not strictly sorted")]
    NonCanonicalInventory,
    /// `rpm` could not start.
    #[error("rpm could not inspect the guest: {0}")]
    Rpm(std::io::Error),
    /// `rpm` returned a failure status.
    #[error("rpm failed with {0}")]
    RpmStatus(std::process::ExitStatus),
    /// `rpm` returned non-UTF-8 output.
    #[error("rpm returned a non-UTF-8 package listing: {0}")]
    RpmUtf8(std::string::FromUtf8Error),
    /// The live package set differs from the lock.
    #[error("the installed package set drifted: {missing} missing, {unexpected} unexpected")]
    PackageDrift {
        /// Locked but absent count.
        missing: usize,
        /// Installed but unlocked count.
        unexpected: usize,
    },
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{HostImageValidator, ImageError, ImageValidator};
    use aex_hands_agent::image_contract::{
        FORBIDDEN_ROOTFS_PATHS, IMAGE_LOCK_PATH, ROOTFS_CONTRACT, RPM_LIST_PATH,
    };

    fn materialize(root: &Path, packages: &[&str]) {
        for entry in ROOTFS_CONTRACT {
            let path = root.join(entry.path.trim_start_matches('/'));
            if entry.directory {
                std::fs::create_dir_all(&path).expect("directory");
            } else {
                std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
                let body = if entry.path == aex_hands_agent::AGENT_SBOM_PATH {
                    br#"{"bomFormat":"CycloneDX"}"#.as_slice()
                } else {
                    b"fixture".as_slice()
                };
                std::fs::write(&path, body).expect("file");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(entry.mode))
                    .expect("mode");
            }
        }
        let listing = format!("{}\n", packages.join("\n"));
        std::fs::write(root.join(RPM_LIST_PATH.trim_start_matches('/')), &listing)
            .expect("listing");
        let lock = aex_hands_agent::ImageLock {
            schema: "aex.hands-image-lock.v1".to_owned(),
            container_base: "example.invalid/base@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            packages: packages.iter().map(|package| (*package).to_owned()).collect(),
        };
        std::fs::write(
            root.join(IMAGE_LOCK_PATH.trim_start_matches('/')),
            serde_json::to_vec(&lock).expect("lock"),
        )
        .expect("lock file");
    }

    #[test]
    fn ready_checks_required_and_forbidden_paths() {
        let temp = tempfile::tempdir().expect("temporary root");
        materialize(temp.path(), &["bash-0:1-1.aarch64"]);
        let validator = HostImageValidator::rooted_at(temp.path().to_path_buf());
        assert!(validator.ready().is_ok());
        let forbidden = temp
            .path()
            .join(FORBIDDEN_ROOTFS_PATHS[0].trim_start_matches('/'));
        std::fs::create_dir_all(&forbidden).expect("forbidden fixture");
        assert!(matches!(
            validator.ready(),
            Err(ImageError::ForbiddenPath { .. })
        ));
    }

    #[test]
    fn validate_requires_both_embedded_inventories_and_the_live_set_to_agree() {
        let temp = tempfile::tempdir().expect("temporary root");
        let packages = ["bash-0:1-1.aarch64", "jq-0:1-1.aarch64"];
        materialize(temp.path(), &packages);
        let validator = HostImageValidator::rooted_at(temp.path().to_path_buf());
        let observed: Vec<String> = packages.iter().map(|value| (*value).to_owned()).collect();
        assert!(validator.validate_documents(&observed).is_ok());
        assert!(matches!(
            validator.validate_documents(&["bash-0:2-1.aarch64".to_owned()]),
            Err(ImageError::PackageDrift { .. })
        ));
    }

    #[test]
    fn ready_refuses_a_malformed_agent_sbom() {
        let temp = tempfile::tempdir().expect("temporary root");
        materialize(temp.path(), &["bash-0:1-1.aarch64"]);
        std::fs::write(
            temp.path()
                .join(aex_hands_agent::AGENT_SBOM_PATH.trim_start_matches('/')),
            br#"{"bomFormat":"not-cyclonedx"}"#,
        )
        .expect("corrupt SBOM");
        let validator = HostImageValidator::rooted_at(temp.path().to_path_buf());
        assert!(matches!(validator.ready(), Err(ImageError::Sbom { .. })));
    }
}
