//! The local build: a generated `Dockerfile`, its build context, and the SBOM
//! layout.
//!
//! # Why two pins and not one
//!
//! The AEX code artifact and the OS layer are reproducible in different senses,
//! and conflating them would be a false guarantee:
//!
//! - the **base image is digest-pinned** ([`BASE_IMAGE_DIGEST`]), so the bytes the
//!   layer is built on cannot move under a tag;
//! - the **package source is date-pinned** ([`RELEASEVER`]), because AL2023 serves
//!   a frozen repository snapshot per `releasever`, so `dnf` resolves the same
//!   NEVRAs tomorrow as it did today;
//! - the resolved NEVRAs are then **locked** in `image.lock.json` and re-checked by
//!   the `/validate` build hook, so a mirror that moves anyway fails the build
//!   instead of silently changing the image.
//!
//! The pins are values read from the registry and the base image itself, not
//! guesses: `docker buildx imagetools inspect` produced the digest and the base's
//! own `/etc/os-release` produced the release date.

use crate::image::{
    AGENT_PATH, ARCHITECTURE, BASE_IMAGE_ARN_TEMPLATE, CONTAINER_BASE, Capability,
    FORBIDDEN_INSTALL_PACKAGES, FORBIDDEN_ROOTFS_PATHS, GUEST_TARGET, HOOK_PORT, JOURNAL_PATH,
    OS_CAPABILITIES, PackageGroup, ROOTFS_CONTRACT, SBOM_DIR, WORKSPACE_PATH,
};
use serde::{Deserialize, Serialize};

/// The machine-readable registration contract carried by every service ZIP.
pub const REGISTRATION_DESCRIPTOR_FILENAME: &str = "microvm-image-registration.json";

/// The schema of [`MicrovmImageRegistration`].
pub const REGISTRATION_SCHEMA: &str = "aex.microvm-image-registration.v1";
use aex_hands_agent::image_contract::{AGENT_SBOM_PATH, IMAGE_LOCK_PATH, RPM_LIST_PATH};

/// The digest the container base is pinned to.
///
/// Read from `public.ecr.aws/lambda/microvms:al2023-minimal` on 2026-08-01. A tag
/// is a moving reference; a digest is the bytes.
pub const BASE_IMAGE_DIGEST: &str =
    "sha256:05cb9b38d841e7ff1b693dc9e894909612f340bf99ec97d426e8000a5bbe96c3";

/// The AL2023 repository snapshot the package set resolves against.
///
/// Read from the pinned base image's own `/etc/os-release`
/// (`PRETTY_NAME="Amazon Linux 2023.12.20260629"`), so the package source and the
/// base image are the same release rather than two that happen to work together.
pub const RELEASEVER: &str = "2023.12.20260629";

/// The epoch every archive entry and file mtime is fixed to.
///
/// `2026-08-01T00:00:00Z`. Without it the artifact digest changes every build,
/// which would make the double-build check pass or fail on the clock.
pub const SOURCE_DATE_EPOCH: u64 = 1_785_628_800;

/// The base image reference, pinned by digest.
#[must_use]
pub fn pinned_base() -> String {
    format!("{CONTAINER_BASE}@{BASE_IMAGE_DIGEST}")
}

/// Which image variant is being built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// The compute shape token.
    pub size: String,
    /// Whether the browser layer is included.
    pub browser: bool,
}

impl Variant {
    /// Parses a variant name such as `1gb` or `4gb-browser`.
    ///
    /// # Errors
    ///
    /// Returns the offered variant names when the name is not one of them.
    pub fn parse(name: &str) -> Result<Self, String> {
        let (size, browser) = name
            .strip_suffix("-browser")
            .map_or((name, false), |size| (size, true));
        let offered = crate::image::variants();
        let matched = offered.iter().any(|variant| {
            variant.size == size && variant.capabilities.contains(&Capability::Browser) == browser
        });
        if matched {
            Ok(Self {
                size: size.to_owned(),
                browser,
            })
        } else {
            Err(format!(
                "`{name}` is not an offered variant; the eight are {}",
                offered
                    .iter()
                    .map(|variant| if variant.capabilities.is_empty() {
                        variant.size.to_owned()
                    } else {
                        format!("{}-browser", variant.size)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }

    /// The image tag this variant builds to.
    #[must_use]
    pub fn tag(&self) -> String {
        if self.browser {
            format!("aex-hands:{}-browser", self.size)
        } else {
            format!("aex-hands:{}", self.size)
        }
    }

    /// The public variant token used by the release manifest and registration record.
    #[must_use]
    pub fn name(&self) -> String {
        if self.browser {
            format!("{}-browser", self.size)
        } else {
            self.size.clone()
        }
    }

    /// The minimum memory the provider must persist on the image version.
    #[must_use]
    pub fn minimum_memory_mib(&self) -> u32 {
        match self.size.as_str() {
            "512mb" => 512,
            "1gb" => 1_024,
            "2gb" => 2_048,
            "4gb" => 4_096,
            "8gb" => 8_192,
            _ => unreachable!("Variant::parse admits exactly five sizes"),
        }
    }

    /// The package groups this variant installs.
    #[must_use]
    pub fn groups(&self) -> Vec<PackageGroup> {
        PackageGroup::ALL
            .into_iter()
            .filter(|group| self.browser || !group.is_browser_layer())
            .collect()
    }

    /// Every package this variant installs, in group order.
    ///
    /// The deleted set is filtered here rather than only asserted in a test: the
    /// install list is what reaches `dnf`, and `curl` in particular aborts the
    /// whole transaction rather than failing on its own.
    #[must_use]
    pub fn packages(&self) -> Vec<&'static str> {
        self.groups()
            .into_iter()
            .flat_map(|group| group.packages().iter().copied())
            .filter(|package| !FORBIDDEN_INSTALL_PACKAGES.contains(package))
            .collect()
    }
}

/// One provider CPU configuration in the registration descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CpuConfiguration {
    /// AWS Lambda `MicroVM` architecture token.
    pub architecture: String,
}

/// One provider resource floor in the registration descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageResources {
    /// Minimum memory accepted by `RunMicrovm` for this variant.
    pub minimum_memory_in_mi_b: u32,
}

/// Guest lifecycle hooks persisted on an image version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MicrovmHooks {
    /// Enables the run hook.
    pub run: String,
    /// Run-hook timeout.
    pub run_timeout_in_seconds: u32,
    /// Enables the resume hook.
    pub resume: String,
    /// Resume-hook timeout.
    pub resume_timeout_in_seconds: u32,
    /// Enables the suspend hook.
    pub suspend: String,
    /// Suspend-hook timeout.
    pub suspend_timeout_in_seconds: u32,
    /// Enables the terminate hook.
    pub terminate: String,
    /// Terminate-hook timeout.
    pub terminate_timeout_in_seconds: u32,
}

/// Image-build hooks persisted on an image version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MicrovmImageHooks {
    /// Enables the ready hook.
    pub ready: String,
    /// Ready-hook timeout.
    pub ready_timeout_in_seconds: u32,
    /// Enables the validate hook.
    pub validate: String,
    /// Validate-hook timeout.
    pub validate_timeout_in_seconds: u32,
}

/// All hooks persisted on an image version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageHooks {
    /// The only port on which the guest serves provider hooks.
    pub port: u16,
    /// Guest lifecycle hooks.
    #[serde(rename = "microvmHooks")]
    pub microvm: MicrovmHooks,
    /// Image-build hooks.
    #[serde(rename = "microvmImageHooks")]
    pub image: MicrovmImageHooks,
}

/// Plane-neutral `CreateMicrovmImage` configuration authenticated by the ZIP digest.
///
/// The private release lane supplies only custody-bound fields: the content-addressed
/// S3 URI, build role, image name, logging destination, tags and client token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MicrovmImageRegistration {
    /// Descriptor schema.
    pub schema: String,
    /// Public variant token.
    pub variant: String,
    /// Whether this is one of the three browser-capable variants.
    pub browser: bool,
    /// Region-templated AWS-managed base image ARN.
    pub base_image_arn_template: String,
    /// Provider CPU configurations.
    pub cpu_configurations: Vec<CpuConfiguration>,
    /// Provider resource floors.
    pub resources: Vec<ImageResources>,
    /// Additional guest OS capabilities.
    pub additional_os_capabilities: Vec<String>,
    /// Provider lifecycle hooks.
    pub hooks: ImageHooks,
    /// Provider-visible variant description.
    pub description: String,
}

/// The plane-neutral registration record packaged with one variant.
#[must_use]
pub fn registration_descriptor(variant: &Variant) -> MicrovmImageRegistration {
    let enabled = "ENABLED".to_owned();
    MicrovmImageRegistration {
        schema: REGISTRATION_SCHEMA.to_owned(),
        variant: variant.name(),
        browser: variant.browser,
        base_image_arn_template: BASE_IMAGE_ARN_TEMPLATE.to_owned(),
        cpu_configurations: vec![CpuConfiguration {
            architecture: ARCHITECTURE.to_owned(),
        }],
        resources: vec![ImageResources {
            minimum_memory_in_mi_b: variant.minimum_memory_mib(),
        }],
        additional_os_capabilities: vec![OS_CAPABILITIES.to_owned()],
        hooks: ImageHooks {
            port: HOOK_PORT,
            microvm: MicrovmHooks {
                run: enabled.clone(),
                run_timeout_in_seconds: 60,
                resume: enabled.clone(),
                resume_timeout_in_seconds: 60,
                suspend: enabled.clone(),
                suspend_timeout_in_seconds: 60,
                terminate: enabled.clone(),
                terminate_timeout_in_seconds: 60,
            },
            image: MicrovmImageHooks {
                ready: enabled.clone(),
                ready_timeout_in_seconds: 300,
                validate: enabled,
                validate_timeout_in_seconds: 120,
            },
        },
        description: format!("aex.variant={}", variant.tag()),
    }
}

/// The generated `Dockerfile` for one variant.
///
/// Every line is derived from the same constants the rootfs contract and the
/// lockfile comparison use, so the built image and the checked contract cannot
/// describe different trees.
#[must_use]
pub fn containerfile(variant: &Variant) -> String {
    use core::fmt::Write as _;

    let packages = variant.packages().join(" ");
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Generated by `hands-image context`. Do not edit.\n\
         # Base pinned by digest; package source pinned to the AL2023 {RELEASEVER} snapshot.\n\
         FROM --platform=linux/arm64 {base}\n\n\
         # AL2023 minimal already supplies `curl-minimal`. Do not replace it with\n\
         # the conflicting full curl package during this noninteractive build.\n\n\
         RUN dnf --releasever={RELEASEVER} --setopt=install_weak_deps=0 -y install \\\n    \
         {packages} \\\n && dnf clean all\n",
        base = pinned_base(),
    );
    let _ = writeln!(out, "RUN mkdir -p {SBOM_DIR}");
    let _ = writeln!(
        out,
        "RUN rpm -qa --qf '%{{NAME}}-%{{EPOCHNUM}}:%{{VERSION}}-%{{RELEASE}}.%{{ARCH}}\\n' | LC_ALL=C sort > {RPM_LIST_PATH}"
    );
    let _ = writeln!(
        out,
        "RUN jq -R -s --arg schema 'aex.hands-image-lock.v1' --arg base '{base}' 'split(\"\\n\") | map(select(length > 0)) | {{schema:$schema,containerBase:$base,packages:.}}' {RPM_LIST_PATH} > {IMAGE_LOCK_PATH} && chmod 0644 {RPM_LIST_PATH} {IMAGE_LOCK_PATH}",
        base = pinned_base(),
    );
    let _ = writeln!(out, "# The rootfs contract the /ready build hook asserts.");
    let _ = writeln!(out, "COPY --chmod=0755 hands-agent {AGENT_PATH}");
    let _ = writeln!(out, "COPY --chmod=0644 agent.cdx.json {AGENT_SBOM_PATH}");
    for entry in ROOTFS_CONTRACT.iter().filter(|entry| entry.directory) {
        let _ = writeln!(
            out,
            "RUN mkdir -p {path} && chmod {mode:o} {path}",
            path = entry.path,
            mode = entry.mode
        );
    }
    let _ = writeln!(
        out,
        "\n# Every artefact the previous design shipped and this one deletes. A\n\
         # build that reintroduces one fails here rather than in a review."
    );
    for forbidden in FORBIDDEN_ROOTFS_PATHS {
        let _ = writeln!(out, "RUN test ! -e {forbidden}");
    }
    let _ = writeln!(
        out,
        "\n# One port: the provider declares exactly one hook port and the endpoint\n\
         # token is scoped to it. There is no shell port.\n\
         ENV AEX_HANDS_LISTEN_ADDR=0.0.0.0:{HOOK_PORT}\n\
         ENV AEX_HANDS_JOURNAL_ROOT={JOURNAL_PATH}\n\
         ENV AEX_HANDS_GUEST_ROOT={WORKSPACE_PATH}\n\
         EXPOSE {HOOK_PORT}\n\
         ENTRYPOINT [\"{AGENT_PATH}\"]"
    );
    out
}

/// The build inputs, rendered for a build record.
#[must_use]
pub fn build_inputs(variant: &Variant) -> Vec<String> {
    vec![
        format!("base={}", pinned_base()),
        format!("releasever={RELEASEVER}"),
        format!("target={GUEST_TARGET}"),
        format!("source_date_epoch={SOURCE_DATE_EPOCH}"),
        format!("variant={}", variant.tag()),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        BASE_IMAGE_DIGEST, RELEASEVER, SOURCE_DATE_EPOCH, Variant, build_inputs, containerfile,
        pinned_base, registration_descriptor,
    };

    #[test]
    fn the_base_is_pinned_by_digest_and_never_by_a_moving_tag() {
        let base = pinned_base();
        assert!(base.contains("@sha256:"), "{base}");
        assert!(BASE_IMAGE_DIGEST.starts_with("sha256:"));
        assert_eq!(
            BASE_IMAGE_DIGEST.len(),
            71,
            "a sha256 reference is `sha256:` plus 64 hex characters"
        );
        let generated = containerfile(&Variant::parse("1gb").expect("an offered variant"));
        assert!(generated.contains(&base));
        assert!(
            !generated.contains("al2023-minimal\n"),
            "the tag alone must never be the FROM reference"
        );
    }

    #[test]
    fn the_package_source_is_date_pinned_to_the_same_release_as_the_base() {
        // Read from the pinned base image's own /etc/os-release, so the two halves
        // of the build are the same AL2023 release rather than two that happen to
        // work together today.
        assert_eq!(RELEASEVER, "2023.12.20260629");
        let generated = containerfile(&Variant::parse("1gb").expect("an offered variant"));
        assert!(
            generated.contains(&format!("--releasever={RELEASEVER}")),
            "an unpinned dnf resolves against whatever the mirror serves today"
        );
        assert!(generated.contains("--setopt=install_weak_deps=0"));
    }

    #[test]
    fn the_base_curl_stays_untouched_and_full_curl_is_never_listed() {
        let generated = containerfile(&Variant::parse("1gb").expect("an offered variant"));
        assert!(!generated.contains("dnf swap"), "{generated}");
        assert!(generated.contains("-y install"), "{generated}");
        assert!(!generated.contains(" curl \\"), "{generated}");
    }

    #[test]
    fn the_browser_layer_is_only_in_the_browser_variants() {
        let base = Variant::parse("4gb").expect("an offered variant");
        let browser = Variant::parse("4gb-browser").expect("an offered variant");
        assert!(!base.packages().contains(&"chromium-headless"));
        assert!(browser.packages().contains(&"chromium-headless"));
        assert_eq!(base.tag(), "aex-hands:4gb");
        assert_eq!(browser.tag(), "aex-hands:4gb-browser");
    }

    #[test]
    fn chromium_is_not_offered_below_a_two_gigabyte_baseline() {
        for name in ["512mb-browser", "1gb-browser"] {
            assert!(
                Variant::parse(name).is_err(),
                "{name} is not one of the eight published variants"
            );
        }
        for name in ["2gb-browser", "4gb-browser", "8gb-browser"] {
            assert!(Variant::parse(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn an_unknown_variant_is_refused_with_the_offered_set() {
        let error = Variant::parse("16gb").expect_err("there is no sixth shape");
        assert!(error.contains("512mb"), "{error}");
        assert!(error.contains("8gb-browser"), "{error}");
    }

    #[test]
    fn the_rootfs_contract_and_the_generated_build_agree_about_every_path() {
        let generated = containerfile(&Variant::parse("1gb").expect("an offered variant"));
        for entry in aex_hands_agent::ROOTFS_CONTRACT {
            assert!(
                generated.contains(entry.path),
                "the build never creates `{}`",
                entry.path
            );
        }
        for forbidden in crate::image::FORBIDDEN_ROOTFS_PATHS {
            assert!(
                generated.contains(&format!("RUN test ! -e {forbidden}")),
                "a build that reintroduces `{forbidden}` must fail here, not in a review"
            );
        }
        assert!(generated.contains("ENTRYPOINT [\"/opt/aex/hands-agent\"]"));
        assert!(
            generated.contains("EXPOSE 8080"),
            "one port, matching the auth-token scope"
        );
    }

    #[test]
    fn the_guest_environment_contract_is_written_by_the_image() {
        let generated = containerfile(&Variant::parse("1gb").expect("an offered variant"));
        for name in aex_hands_agent::boot::REQUIRED_VARS {
            assert!(
                generated.contains(&format!("ENV {name}=")),
                "the guest refuses to start without `{name}`, so the image must write it"
            );
        }
        assert!(
            !generated.contains("AWS_"),
            "no credential has anywhere to arrive"
        );
    }

    #[test]
    fn the_build_inputs_are_the_ones_a_second_build_would_have_to_match() {
        let inputs = build_inputs(&Variant::parse("1gb").expect("an offered variant"));
        assert!(inputs.iter().any(|input| input.contains(BASE_IMAGE_DIGEST)));
        assert!(inputs.iter().any(|input| input.contains(RELEASEVER)));
        assert!(
            inputs
                .iter()
                .any(|input| input.contains("aarch64-unknown-linux-musl"))
        );
        assert!(
            inputs
                .iter()
                .any(|input| input == &format!("source_date_epoch={SOURCE_DATE_EPOCH}")),
            "without a fixed epoch the artifact digest changes on the clock"
        );
    }

    #[test]
    fn the_registration_descriptor_is_an_exact_provider_configuration() {
        let descriptor =
            registration_descriptor(&Variant::parse("8gb-browser").expect("an offered variant"));
        assert_eq!(descriptor.schema, "aex.microvm-image-registration.v1");
        assert_eq!(descriptor.variant, "8gb-browser");
        assert!(descriptor.browser);
        assert_eq!(descriptor.resources[0].minimum_memory_in_mi_b, 8_192);
        assert_eq!(descriptor.cpu_configurations[0].architecture, "ARM_64");
        assert_eq!(descriptor.additional_os_capabilities, ["ALL"]);
        assert_eq!(descriptor.hooks.port, 8_080);
        assert_eq!(descriptor.hooks.microvm.run, "ENABLED");
        assert_eq!(descriptor.hooks.microvm.run_timeout_in_seconds, 60);
        assert_eq!(descriptor.hooks.image.ready, "ENABLED");
        assert_eq!(descriptor.hooks.image.ready_timeout_in_seconds, 300);
        assert_eq!(descriptor.hooks.image.validate_timeout_in_seconds, 120);
        assert!(
            serde_json::to_string(&descriptor)
                .expect("fixed descriptor serializes")
                .contains("\"minimumMemoryInMiB\":8192")
        );
    }

    #[test]
    fn the_sbom_names_the_three_inventories_a_customer_can_read_back() {
        let names = [
            aex_hands_agent::AGENT_SBOM_PATH,
            aex_hands_agent::RPM_LIST_PATH,
            aex_hands_agent::IMAGE_LOCK_PATH,
        ];
        assert_eq!(
            names,
            [
                "/opt/aex/sbom/agent.cdx.json",
                "/opt/aex/sbom/rpm-nevra.txt",
                "/opt/aex/sbom/image.lock.json"
            ]
        );
    }
}
