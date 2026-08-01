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

use serde::{Deserialize, Serialize};

/// The AWS-managed base image the AEX layer sits on.
pub const BASE_IMAGE_ARN_TEMPLATE: &str = "arn:aws:lambda:{region}:aws:microvm-image:al2023-1";

/// The container base the AEX layer is built from.
pub const CONTAINER_BASE: &str = "public.ecr.aws/lambda/microvms:al2023-minimal";

/// The only architecture offered.
pub const ARCHITECTURE: &str = "ARM_64";

/// The guest target triple. Static, so an ordinary customer `pip`, `dnf` or
/// `ldconfig` cannot break the supervisor out from under its own operation.
pub const GUEST_TARGET: &str = "aarch64-unknown-linux-musl";

/// Where the agent binary lives.
pub const AGENT_PATH: &str = "/opt/aex/hands-agent";

/// Where the SBOM lives.
pub const SBOM_DIR: &str = "/opt/aex/sbom";

/// The guest root.
pub const WORKSPACE_PATH: &str = "/workspace";

/// The operation journal root.
pub const JOURNAL_PATH: &str = "/var/lib/aex/hands";

/// The single hook port.
pub const HOOK_PORT: u16 = 8_080;

/// Additional OS capabilities.
///
/// Retained — but no longer for a firewall. Customers legitimately need
/// namespaces, mounts and containers inside their own VM, and H-BOUNDARY already
/// grants them root, so withholding capabilities would inconvenience them without
/// protecting anything.
pub const OS_CAPABILITIES: &str = "ALL";

/// A capability layer an image variant may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Headless Chromium plus its font and NSS dependencies.
    Browser,
}

/// A package group in the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PackageGroup {
    /// Shell and core utilities.
    ShellCore,
    /// Archive and network tools.
    ArchiveNet,
    /// Language toolchains.
    Languages,
    /// Search tools.
    Search,
    /// The browser layer.
    Browser,
}

impl PackageGroup {
    /// Every group.
    pub const ALL: [Self; 5] = [
        Self::ShellCore,
        Self::ArchiveNet,
        Self::Languages,
        Self::Search,
        Self::Browser,
    ];

    /// The packages in this group.
    ///
    /// **Customer tooling only.** Nothing here is on an AEX trust path: the agent
    /// is a static binary with no interpreter dependency, which the language-free
    /// image variant proves by passing the whole protocol and filesystem suite.
    #[must_use]
    pub const fn packages(self) -> &'static [&'static str] {
        match self {
            Self::ShellCore => &[
                "bash",
                "coreutils",
                "findutils",
                "grep",
                "sed",
                "gawk",
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
                "ca-certificates",
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
            Self::Search => &["ripgrep"],
            Self::Browser => &[
                "chromium-headless",
                "nss",
                "liberation-fonts",
                "dejavu-sans-fonts",
            ],
        }
    }

    /// Whether this group is only present in a browser variant.
    #[must_use]
    pub const fn is_browser_layer(self) -> bool {
        matches!(self, Self::Browser)
    }
}

/// The `curl` swap that must run **before** any install.
///
/// This base's `dnf` is a symlink to `microdnf`, which has no `--allowerasing`, so
/// listing `curl` in the install set aborts the whole transaction with
/// `curl-minimal conflicts with curl`. That is a real one-minute
/// `CreateMicrovmImage` failure observed on 2026-07-18, not a precaution.
pub const CURL_SWAP: &str = "dnf swap curl-minimal curl";

/// Packages that must never appear in an install list.
pub const FORBIDDEN_INSTALL_PACKAGES: [&str; 5] = [
    // See `CURL_SWAP`.
    "curl",
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

/// One pinned package, by exact NEVRA.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PinnedPackage {
    /// The package name.
    pub name: String,
    /// The exact `name-epoch:version-release.arch` string.
    pub nevra: String,
}

/// The image lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ImageLock {
    /// Lockfile schema version.
    pub version: u32,
    /// The container base this lock was resolved against.
    pub container_base: String,
    /// The pinned package set, sorted by NEVRA.
    pub packages: Vec<PinnedPackage>,
}

impl ImageLock {
    /// Whether an observed `rpm -qa` set matches the lock exactly.
    ///
    /// Both directions matter: a missing package is a broken image and an extra one
    /// is an unreviewed dependency, so `/validate` returns 503 for either.
    #[must_use]
    pub fn matches(&self, observed: &[String]) -> LockVerdict {
        let mut expected: Vec<&str> = self
            .packages
            .iter()
            .map(|package| package.nevra.as_str())
            .collect();
        expected.sort_unstable();
        let mut seen: Vec<&str> = observed.iter().map(String::as_str).collect();
        seen.sort_unstable();

        let missing: Vec<String> = expected
            .iter()
            .filter(|nevra| !seen.contains(nevra))
            .map(|nevra| (*nevra).to_owned())
            .collect();
        let unexpected: Vec<String> = seen
            .iter()
            .filter(|nevra| !expected.contains(nevra))
            .map(|nevra| (*nevra).to_owned())
            .collect();
        if missing.is_empty() && unexpected.is_empty() {
            LockVerdict::Match
        } else {
            LockVerdict::Drift {
                missing,
                unexpected,
            }
        }
    }
}

/// What a lockfile comparison found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockVerdict {
    /// The installed set is exactly the lock.
    Match,
    /// The mirror moved. The build fails rather than shipping a changed image.
    Drift {
        /// Locked but not installed.
        missing: Vec<String>,
        /// Installed but not locked.
        unexpected: Vec<String>,
    },
}

impl LockVerdict {
    /// The HTTP status the `/validate` build hook returns.
    #[must_use]
    pub const fn http_status(&self) -> u16 {
        match self {
            Self::Match => 200,
            Self::Drift { .. } => 503,
        }
    }
}

/// One published image variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageVariant {
    /// The compute shape it is built for.
    pub size: &'static str,
    /// Its capability layers.
    pub capabilities: Vec<Capability>,
    /// `minimumMemoryInMiB` for `CreateMicrovmImage`.
    pub minimum_memory_mib: u32,
}

/// Every variant published per region.
///
/// Five base images plus browser images for the three shapes that can carry
/// Chromium: eight per region, inside the hundred-image account limit.
#[must_use]
pub fn variants() -> Vec<ImageVariant> {
    let shapes: [(&str, u32, bool); 5] = [
        ("512mb", 512, false),
        ("1gb", 1_024, false),
        ("2gb", 2_048, true),
        ("4gb", 4_096, true),
        ("8gb", 8_192, true),
    ];
    let mut out = Vec::new();
    for (size, memory, browser) in shapes {
        out.push(ImageVariant {
            size,
            capabilities: Vec::new(),
            minimum_memory_mib: memory,
        });
        if browser {
            out.push(ImageVariant {
                size,
                capabilities: vec![Capability::Browser],
                minimum_memory_mib: memory,
            });
        }
    }
    out
}

/// A path the rootfs contract requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootfsEntry {
    /// The path.
    pub path: &'static str,
    /// The required mode.
    pub mode: u32,
    /// Whether it is a directory.
    pub directory: bool,
}

/// The rootfs contract the `/ready` build hook asserts.
pub const ROOTFS_CONTRACT: [RootfsEntry; 4] = [
    RootfsEntry {
        path: AGENT_PATH,
        mode: 0o755,
        directory: false,
    },
    RootfsEntry {
        path: SBOM_DIR,
        mode: 0o755,
        directory: true,
    },
    RootfsEntry {
        path: WORKSPACE_PATH,
        mode: 0o755,
        directory: true,
    },
    RootfsEntry {
        path: JOURNAL_PATH,
        mode: 0o755,
        directory: true,
    },
];

/// Paths and artefacts that must **not** be in the image.
///
/// Each one is something the previous design shipped and this one deletes, so the
/// scan is a regression guard rather than a generic hygiene sweep.
pub const FORBIDDEN_ROOTFS_PATHS: [&str; 8] = [
    // Deleted: there is no Bun or Node trusted runtime.
    "/opt/aex/customer/bun",
    "/opt/aex/customer/python",
    "/opt/aex/runner-bundle",
    // Deleted with per-uid isolation: H-BOUNDARY grants real root, so there are no
    // other uids and no launcher to drop into them.
    "/opt/aex/customer-process-isolation",
    // Deleted with the in-guest firewall.
    "/etc/nftables",
    "/etc/nftables.conf",
    // A guest holds no credential of any kind.
    "/root/.aws",
    "/opt/aex/credentials",
];

#[cfg(test)]
mod tests {
    use super::{
        AGENT_PATH, ARCHITECTURE, CURL_SWAP, Capability, FORBIDDEN_INSTALL_PACKAGES,
        FORBIDDEN_ROOTFS_PATHS, GUEST_TARGET, HOOK_PORT, ImageLock, LockVerdict, OS_CAPABILITIES,
        PackageGroup, PinnedPackage, ROOTFS_CONTRACT, variants,
    };

    fn lock(nevras: &[&str]) -> ImageLock {
        ImageLock {
            version: 1,
            container_base: super::CONTAINER_BASE.to_owned(),
            packages: nevras
                .iter()
                .map(|nevra| PinnedPackage {
                    name: nevra.split('-').next().unwrap_or(nevra).to_owned(),
                    nevra: (*nevra).to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn eight_variants_are_published_per_region() {
        let published = variants();
        assert_eq!(published.len(), 8, "five base plus three browser variants");
        let browser: Vec<&str> = published
            .iter()
            .filter(|variant| variant.capabilities.contains(&Capability::Browser))
            .map(|variant| variant.size)
            .collect();
        assert_eq!(
            browser,
            vec!["2gb", "4gb", "8gb"],
            "Chromium is not offered below a 2 GiB baseline"
        );
        assert!(published.len() <= 100, "inside the account image limit");
    }

    #[test]
    fn every_variant_declares_its_minimum_memory() {
        let expected = [512, 1_024, 2_048, 2_048, 4_096, 4_096, 8_192, 8_192];
        let observed: Vec<u32> = variants()
            .iter()
            .map(|variant| variant.minimum_memory_mib)
            .collect();
        assert_eq!(observed, expected);
    }

    #[test]
    fn curl_is_swapped_and_never_installed() {
        assert!(CURL_SWAP.starts_with("dnf swap curl-minimal curl"));
        for group in PackageGroup::ALL {
            assert!(
                !group.packages().contains(&"curl"),
                "{group:?} lists curl, which aborts the whole microdnf transaction"
            );
        }
        assert!(FORBIDDEN_INSTALL_PACKAGES.contains(&"curl"));
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
    fn the_browser_layer_is_its_own_group() {
        assert!(PackageGroup::Browser.is_browser_layer());
        for group in PackageGroup::ALL {
            assert_eq!(
                group.is_browser_layer(),
                group == PackageGroup::Browser,
                "{group:?}"
            );
        }
        assert!(
            PackageGroup::Browser
                .packages()
                .contains(&"chromium-headless")
        );
    }

    #[test]
    fn a_matching_lockfile_validates_and_a_mutated_one_fails_the_build() {
        let locked = lock(&[
            "bash-0:5.2.15-1.amzn2023.aarch64",
            "jq-0:1.7-1.amzn2023.aarch64",
        ]);
        let installed = vec![
            "jq-0:1.7-1.amzn2023.aarch64".to_owned(),
            "bash-0:5.2.15-1.amzn2023.aarch64".to_owned(),
        ];
        assert_eq!(locked.matches(&installed), LockVerdict::Match);
        assert_eq!(LockVerdict::Match.http_status(), 200);

        // A mirror moved: the release bumped underneath us.
        let drifted = vec![
            "jq-0:1.7-1.amzn2023.aarch64".to_owned(),
            "bash-0:5.2.15-2.amzn2023.aarch64".to_owned(),
        ];
        let verdict = locked.matches(&drifted);
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
        let verdict = locked.matches(&installed);
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
                r#"{"version":1,"containerBase":"x","packages":[],"extra":1}"#
            )
            .is_err()
        );
    }

    #[test]
    fn the_rootfs_contract_names_the_four_paths_the_ready_hook_asserts() {
        let paths: Vec<&str> = ROOTFS_CONTRACT.iter().map(|entry| entry.path).collect();
        assert_eq!(
            paths,
            vec![
                "/opt/aex/hands-agent",
                "/opt/aex/sbom",
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
