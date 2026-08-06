//! The `hands-image` rootfs contract, package manifest and build definition.
//!
//! Two honest halves, and they are not the same claim:
//!
//! - **the AEX code artifact is byte-reproducible.** `cargo build --locked` for
//!   `aarch64-unknown-linux-musl` with `SOURCE_DATE_EPOCH`, `--remap-path-prefix`,
//!   no build-time network, and a ZIP writer that sorts entries and fixes mtime;
//! - **the OS layer is pinned, not reproducible.** AL2023 repositories move. Every
//!   package is pinned by exact NEVRA in `image.lock.json`, and the `/validate`
//!   build hook compares `rpm -qa` against the lockfile so a drifting mirror fails
//!   the build instead of silently changing the image.
//!
//! Claiming reproducibility for the OS layer would be a false guarantee; a checked
//! lockfile is a real one.

pub use aex_hands_agent::image_contract::{
    AGENT_PATH, FORBIDDEN_ROOTFS_PATHS, ImageLock, LockVerdict, ROOTFS_CONTRACT, SBOM_DIR,
};

/// The AWS-managed base image the AEX layer sits on.
pub const BASE_IMAGE_ARN_TEMPLATE: &str = "arn:aws:lambda:{region}:aws:microvm-image:al2023-1";

/// The container base the AEX layer is built from.
pub const CONTAINER_BASE: &str = "public.ecr.aws/lambda/microvms:al2023-minimal";

/// The only architecture offered.
pub const ARCHITECTURE: &str = "ARM_64";

/// The guest target triple. Static, so an ordinary customer `pip`, `dnf` or
/// `ldconfig` cannot break the supervisor out from under its own operation.
pub const GUEST_TARGET: &str = "aarch64-unknown-linux-musl";

/// The guest root.
pub const WORKSPACE_PATH: &str = aex_hands_agent::boot::GUEST_ROOT;

/// The operation journal root.
pub const JOURNAL_PATH: &str = aex_hands_agent::boot::JOURNAL_ROOT;

/// The single hook port.
pub const HOOK_PORT: u16 = 8_080;

/// Additional OS capabilities.
///
/// Retained — but no longer for a firewall. Customers legitimately need
/// namespaces, mounts and containers inside their own VM, and H-BOUNDARY already
/// grants them root, so withholding capabilities would inconvenience them without
/// protecting anything.
pub const OS_CAPABILITIES: &str = "ALL";

/// A package group in the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PackageGroup {
    /// Shell and core utilities.
    ShellCore,
    /// Archive and network tools.
    ArchiveNet,
    /// Language toolchains.
    Languages,
}

impl PackageGroup {
    /// Every group.
    pub const ALL: [Self; 3] = [Self::ShellCore, Self::ArchiveNet, Self::Languages];

    /// The packages in this group.
    ///
    /// **Customer tooling only.** Nothing here is on an AEX trust path: the agent
    /// is a static binary with no interpreter dependency, which the language-free
    /// image variant proves by passing the whole protocol and filesystem suite.
    #[must_use]
    pub const fn packages(self) -> &'static [&'static str] {
        match self {
            Self::ShellCore => &[
                "findutils",
                "which",
                "less",
                "procps-ng",
                "file",
                "diffutils",
                "patch",
            ],
            Self::ArchiveNet => &[
                "tar",
                "zstd",
                "gzip",
                "unzip",
                "zip",
                "wget",
                "jq",
                "openssh-clients",
            ],
            Self::Languages => &[
                "python3",
                "python3-pip",
                "nodejs",
                "npm",
                "git",
                "make",
                "gcc",
                "gcc-c++",
            ],
        }
    }
}

/// Packages that must never appear in an install list.
pub const FORBIDDEN_INSTALL_PACKAGES: [&str; 11] = [
    // These exact packages are already installed in the pinned AL2023 base.
    // Re-requesting them adds no capability and needlessly lets the transaction
    // reconsider the base package set.
    "bash",
    "grep",
    "sed",
    "gawk",
    "ca-certificates",
    // AL2023 minimal already supplies `curl-minimal`; installing the legacy
    // `curl` name conflicts with it.
    "curl",
    // AL2023 minimal already supplies `coreutils-single`; installing the full
    // `coreutils` package conflicts with it and aborts the image transaction.
    "coreutils",
    // Deleted with the in-guest firewall: root can flush any nft table, and the
    // IMDS rule protected an execution role this target never attaches.
    "nftables",
    "iproute",
    // The uid-isolation launcher is deleted, not ported: there are no other uids.
    "shadow-utils",
    // There is no Node or Bun trusted runtime; `nodejs` ships as a customer tool
    // and this vendored binary does not ship at all.
    "bun",
];

/// One published image variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageVariant {
    /// The compute shape it is built for.
    pub size: &'static str,
    /// `minimumMemoryInMiB` for `CreateMicrovmImage`.
    pub minimum_memory_mib: u32,
}

/// The five non-browser variants the local generator and release authority offer.
#[must_use]
pub fn variants() -> Vec<ImageVariant> {
    let shapes: [(&str, u32); 5] = [
        ("512mb", 512),
        ("1gb", 1_024),
        ("2gb", 2_048),
        ("4gb", 4_096),
        ("8gb", 8_192),
    ];
    shapes
        .into_iter()
        .map(|(size, minimum_memory_mib)| ImageVariant {
            size,
            minimum_memory_mib,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        AGENT_PATH, ARCHITECTURE, FORBIDDEN_INSTALL_PACKAGES, FORBIDDEN_ROOTFS_PATHS, GUEST_TARGET,
        HOOK_PORT, ImageLock, LockVerdict, OS_CAPABILITIES, PackageGroup, ROOTFS_CONTRACT,
        variants,
    };

    fn lock(nevras: &[&str]) -> ImageLock {
        ImageLock {
            schema: "aex.hands-image-lock.v1".to_owned(),
            container_base: format!(
                "{}@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                super::CONTAINER_BASE
            ),
            packages: nevras.iter().map(|nevra| (*nevra).to_owned()).collect(),
        }
    }

    #[test]
    fn five_non_browser_variants_are_published_per_region() {
        let published = variants();
        assert_eq!(published.len(), 5);
        assert_eq!(
            published
                .iter()
                .map(|variant| variant.size)
                .collect::<Vec<_>>(),
            vec!["512mb", "1gb", "2gb", "4gb", "8gb"]
        );
    }

    #[test]
    fn every_variant_declares_its_minimum_memory() {
        let expected = [512, 1_024, 2_048, 4_096, 8_192];
        let observed: Vec<u32> = variants()
            .iter()
            .map(|variant| variant.minimum_memory_mib)
            .collect();
        assert_eq!(observed, expected);
    }

    #[test]
    fn the_conflicting_full_curl_package_is_never_installed() {
        for group in PackageGroup::ALL {
            assert!(
                !group.packages().contains(&"curl"),
                "{group:?} lists curl, which aborts the whole microdnf transaction"
            );
        }
        assert!(FORBIDDEN_INSTALL_PACKAGES.contains(&"curl"));
    }

    #[test]
    fn the_conflicting_full_coreutils_package_is_never_installed() {
        for group in PackageGroup::ALL {
            assert!(
                !group.packages().contains(&"coreutils"),
                "{group:?} lists coreutils, which conflicts with the base coreutils-single package"
            );
        }
        assert!(FORBIDDEN_INSTALL_PACKAGES.contains(&"coreutils"));
    }

    #[test]
    fn the_pinned_base_packages_are_never_reinstalled() {
        for package in ["bash", "grep", "sed", "gawk", "ca-certificates"] {
            assert!(FORBIDDEN_INSTALL_PACKAGES.contains(&package), "{package}");
            for group in PackageGroup::ALL {
                assert!(
                    !group.packages().contains(&package),
                    "{group:?} redundantly requests base-installed `{package}`"
                );
            }
        }
    }

    #[test]
    fn the_pinned_al2023_snapshot_does_not_request_unavailable_ripgrep() {
        for group in PackageGroup::ALL {
            assert!(
                !group.packages().contains(&"ripgrep"),
                "{group:?} requests ripgrep, which the pinned AL2023 snapshot does not publish"
            );
        }
    }

    #[test]
    fn the_deleted_packages_are_absent_from_every_group() {
        for group in PackageGroup::ALL {
            for forbidden in FORBIDDEN_INSTALL_PACKAGES {
                assert!(
                    !group.packages().contains(&forbidden),
                    "{group:?} still installs `{forbidden}`"
                );
            }
        }
    }

    #[test]
    fn the_language_toolchains_are_customer_tools_and_nothing_else() {
        // They live in exactly one group, so the language-free variant is one group
        // removal rather than a scattered edit.
        let languages = PackageGroup::Languages.packages();
        for interpreter in ["python3", "nodejs", "npm", "git"] {
            assert!(languages.contains(&interpreter), "{interpreter}");
            let elsewhere = PackageGroup::ALL
                .into_iter()
                .filter(|group| *group != PackageGroup::Languages)
                .any(|group| group.packages().contains(&interpreter));
            assert!(
                !elsewhere,
                "{interpreter} appears outside the language group"
            );
        }
    }

    #[test]
    fn no_published_package_group_contains_a_browser() {
        for group in PackageGroup::ALL {
            assert!(
                !group.packages().contains(&"chromium-headless"),
                "{group:?}"
            );
        }
    }

    #[test]
    fn a_matching_lockfile_validates_and_a_mutated_one_fails_the_build() {
        let locked = lock(&[
            "bash-0:5.2.15-1.amzn2023.aarch64",
            "jq-0:1.7-1.amzn2023.aarch64",
        ]);
        let installed = vec![
            "bash-0:5.2.15-1.amzn2023.aarch64".to_owned(),
            "jq-0:1.7-1.amzn2023.aarch64".to_owned(),
        ];
        assert_eq!(locked.validate(), Ok(()));
        assert_eq!(locked.compare(&installed), LockVerdict::Match);
        assert_eq!(LockVerdict::Match.http_status(), 200);

        // A mirror moved: the release bumped underneath us.
        let drifted = vec![
            "jq-0:1.7-1.amzn2023.aarch64".to_owned(),
            "bash-0:5.2.15-2.amzn2023.aarch64".to_owned(),
        ];
        let verdict = locked.compare(&drifted);
        assert_eq!(
            verdict.http_status(),
            503,
            "a drifting mirror fails the build"
        );
        let LockVerdict::Drift {
            missing,
            unexpected,
        } = verdict
        else {
            panic!("drift must be reported as drift");
        };
        assert_eq!(missing, vec!["bash-0:5.2.15-1.amzn2023.aarch64"]);
        assert_eq!(unexpected, vec!["bash-0:5.2.15-2.amzn2023.aarch64"]);
    }

    #[test]
    fn an_extra_installed_package_is_drift_too() {
        let locked = lock(&["bash-0:5.2.15-1.amzn2023.aarch64"]);
        let installed = vec![
            "bash-0:5.2.15-1.amzn2023.aarch64".to_owned(),
            "nftables-0:1.0.4-1.amzn2023.aarch64".to_owned(),
        ];
        let verdict = locked.compare(&installed);
        assert_eq!(
            verdict.http_status(),
            503,
            "an unreviewed dependency is as much a build failure as a missing one"
        );
    }

    #[test]
    fn the_lockfile_round_trips_through_json() {
        let locked = lock(&["bash-0:5.2.15-1.amzn2023.aarch64"]);
        let json = serde_json::to_string(&locked).expect("the lock serializes");
        let restored: ImageLock = serde_json::from_str(&json).expect("the lock deserializes");
        assert_eq!(restored, locked);
        // An unknown key is refused, so a hand-edited lockfile fails loudly.
        assert!(
            serde_json::from_str::<ImageLock>(
                r#"{"schema":"aex.hands-image-lock.v1","containerBase":"x@sha256:y","packages":["x"],"extra":1}"#
            )
            .is_err()
        );
    }

    #[test]
    fn the_rootfs_contract_names_every_path_the_ready_hook_asserts() {
        let paths: Vec<&str> = ROOTFS_CONTRACT.iter().map(|entry| entry.path).collect();
        assert_eq!(
            paths,
            vec![
                "/opt/aex/hands-agent",
                "/opt/aex/sbom",
                "/opt/aex/sbom/agent.cdx.json",
                "/opt/aex/sbom/rpm-nevra.txt",
                "/opt/aex/sbom/image.lock.json",
                "/workspace",
                "/var/lib/aex/hands"
            ]
        );
        assert_eq!(AGENT_PATH, "/opt/aex/hands-agent");
        let agent = ROOTFS_CONTRACT[0];
        assert_eq!(agent.mode, 0o755);
        assert!(!agent.directory);
    }

    #[test]
    fn the_deleted_artefacts_have_no_path_in_the_contract() {
        for forbidden in FORBIDDEN_ROOTFS_PATHS {
            assert!(
                !ROOTFS_CONTRACT
                    .iter()
                    .any(|entry| entry.path.starts_with(forbidden)),
                "`{forbidden}` reappeared in the rootfs contract"
            );
        }
        // The credential negatives are stated explicitly, because "no credential"
        // is the control a reader will look for.
        assert!(FORBIDDEN_ROOTFS_PATHS.contains(&"/root/.aws"));
    }

    #[test]
    fn the_rootfs_paths_are_the_ones_the_guest_binary_reads() {
        // Two packages have to agree about where the journal and the workspace
        // live: the binary reads them from its environment and the image writes
        // them. Both take the values from `aex_hands_agent::boot`, and this is the
        // assertion that keeps the third copy — the rootfs contract — in step.
        assert_eq!(super::JOURNAL_PATH, aex_hands_agent::boot::JOURNAL_ROOT);
        assert_eq!(super::WORKSPACE_PATH, aex_hands_agent::boot::GUEST_ROOT);
        assert_eq!(
            format!("0.0.0.0:{}", super::HOOK_PORT),
            aex_hands_agent::boot::LISTEN_ADDR
        );
    }

    #[test]
    fn the_build_inputs_are_pinned() {
        assert_eq!(ARCHITECTURE, "ARM_64");
        assert_eq!(GUEST_TARGET, "aarch64-unknown-linux-musl");
        assert_eq!(
            HOOK_PORT, 8_080,
            "one hook port, matching the auth-token scope"
        );
        assert_eq!(
            OS_CAPABILITIES, "ALL",
            "retained for customer capability, not for a firewall that no longer exists"
        );
    }
}
