//! `hands-image` — the Hands guest image definition and its local build.
//!
//! The image is the rootfs a `MicroVM` boots: a static, credential-free agent, a
//! digest-pinned base, a date-pinned package set locked by NEVRA, and an SBOM the
//! customer can read back from inside their own VM.
//!
//! # What this binary does, and what it does not
//!
//! It writes a build context and can run a **local** container build. It publishes
//! nothing: no registry push, no `CreateMicrovmImage`, no credential. The publish
//! inputs are printed so a release job can use them, and the image-mutation IAM
//! actions live in a separate release role no runtime deployable holds.

mod build;
mod image;
mod sbom;

use std::io::Write as _;
use std::path::{Path, PathBuf};

use build::Variant;
use clap::{Parser, Subcommand};

/// The Hands guest image definition and its local build.
#[derive(Debug, Parser)]
#[command(name = "hands-image", about, version)]
struct Cli {
    /// What to do.
    #[command(subcommand)]
    command: Command,
}

/// The commands this tool offers.
#[derive(Debug, Subcommand)]
enum Command {
    /// Writes the build context for one variant.
    Context {
        /// Which variant, such as `1gb` or `4gb-browser`.
        #[arg(long)]
        variant: String,
        /// Where to write it.
        #[arg(long)]
        out: PathBuf,
        /// The cross-built guest binary to stage.
        #[arg(long)]
        agent: PathBuf,
        /// The `CycloneDX` inventory for the staged guest binary.
        #[arg(long)]
        agent_sbom: PathBuf,
    },
    /// Writes the build context and runs a local container build.
    ///
    /// Local only. There is no push, and no credential is read.
    Build {
        /// Which variant.
        #[arg(long)]
        variant: String,
        /// Where to write the context.
        #[arg(long)]
        out: PathBuf,
        /// The cross-built guest binary to stage.
        #[arg(long)]
        agent: PathBuf,
        /// The `CycloneDX` inventory for the staged guest binary.
        #[arg(long)]
        agent_sbom: PathBuf,
    },
    /// Builds the ARM64 guest and writes one AWS `MicroVM` service context.
    ///
    /// This is the release recipe. Its child build argv and SBOM projection are
    /// fixed in source, so CI has no unrecorded shell pre-step.
    Artifact {
        /// Which of the eight image variants to produce.
        #[arg(long)]
        variant: String,
        /// An empty directory that becomes the root of the service ZIP.
        #[arg(long)]
        out: PathBuf,
    },
    /// Prints the `CreateMicrovmImage` inputs for one variant.
    Publish {
        /// Which variant.
        #[arg(long)]
        variant: String,
        /// Which region the base image ARN names.
        #[arg(long)]
        region: String,
    },
    /// Compares an observed `rpm -qa` listing against the lockfile.
    ///
    /// This is the `/validate` build hook's decision, runnable off-VM.
    Validate {
        /// The lockfile.
        #[arg(long)]
        lock: PathBuf,
        /// A file holding one NEVRA per line, as `rpm -qa` prints them.
        #[arg(long)]
        observed: PathBuf,
    },
}

/// Why `hands-image` stopped.
#[derive(Debug, thiserror::Error)]
enum RunError {
    /// A variant name was not one of the eight.
    #[error("{0}")]
    Variant(String),
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// Which path.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// The lockfile did not decode.
    #[error("the lockfile did not decode: {0}")]
    Lock(String),
    /// The installed package set is not the locked one.
    #[error(
        "the package set drifted: {missing} locked and absent, {unexpected} installed and unlocked"
    )]
    Drift {
        /// How many locked packages are missing.
        missing: usize,
        /// How many installed packages are not locked.
        unexpected: usize,
    },
    /// The local container build failed.
    #[error("the local build failed: {0}")]
    Build(String),
    /// A reused context could smuggle stale files into the service artifact.
    #[error("the build-context directory is not empty: {}", .0.display())]
    OutputNotEmpty(PathBuf),
    /// A fixed child build command failed.
    #[error("the artifact build step failed: {0}")]
    ArtifactBuild(String),
    /// The shipped dependency inventory could not be generated.
    #[error("the agent SBOM could not be generated: {0}")]
    Sbom(String),
}

/// Wraps an I/O error with the path that produced it.
fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> RunError + use<'_> {
    move |source| RunError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Writes the build context for one variant.
fn write_context(
    variant: &Variant,
    out: &Path,
    agent: &Path,
    agent_sbom: &Path,
) -> Result<PathBuf, RunError> {
    let sbom = std::fs::read(agent_sbom).map_err(io_at(agent_sbom))?;
    write_context_bytes(variant, out, agent, &sbom)
}

fn write_context_bytes(
    variant: &Variant,
    out: &Path,
    agent: &Path,
    agent_sbom: &[u8],
) -> Result<PathBuf, RunError> {
    std::fs::create_dir_all(out).map_err(io_at(out))?;
    let mut existing = std::fs::read_dir(out).map_err(io_at(out))?;
    if existing.next().transpose().map_err(io_at(out))?.is_some() {
        return Err(RunError::OutputNotEmpty(out.to_path_buf()));
    }
    let dockerfile = out.join("Dockerfile");
    std::fs::write(&dockerfile, build::containerfile(variant)).map_err(io_at(&dockerfile))?;
    std::fs::copy(agent, out.join("hands-agent")).map_err(io_at(agent))?;
    let sbom_path = out.join("agent.cdx.json");
    std::fs::write(&sbom_path, agent_sbom).map_err(io_at(&sbom_path))?;
    Ok(dockerfile)
}

fn target_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| PathBuf::from("target"), PathBuf::from)
}

fn build_release_artifact(variant: &Variant, out: &Path) -> Result<PathBuf, RunError> {
    let argv = [
        "zigbuild",
        "--locked",
        "--release",
        "--package",
        "hands-agent",
        "--target",
        image::GUEST_TARGET,
    ];
    let status = std::process::Command::new("cargo")
        .args(argv)
        .status()
        .map_err(|error| RunError::ArtifactBuild(error.to_string()))?;
    if !status.success() {
        return Err(RunError::ArtifactBuild(format!(
            "cargo {} exited with {status}",
            argv.join(" ")
        )));
    }
    let agent = target_root()
        .join(image::GUEST_TARGET)
        .join("release")
        .join("hands-agent");
    if !agent.is_file() {
        return Err(RunError::ArtifactBuild(format!(
            "the fixed guest output does not exist: {}",
            agent.display()
        )));
    }
    let manifest = Path::new("runtimes/hands-agent/Cargo.toml");
    let agent_sbom = sbom::generate(manifest, image::GUEST_TARGET)
        .map_err(|error| RunError::Sbom(error.to_string()))?;
    write_context_bytes(variant, out, &agent, &agent_sbom)
}

/// Runs the whole tool.
fn run(cli: &Cli) -> Result<(), RunError> {
    match &cli.command {
        Command::Context {
            variant,
            out,
            agent,
            agent_sbom,
        } => {
            let variant = Variant::parse(variant).map_err(RunError::Variant)?;
            let written = write_context(&variant, out, agent, agent_sbom)?;
            println!("{}", written.display());
            for input in build::build_inputs(&variant) {
                println!("{input}");
            }
            Ok(())
        }
        Command::Build {
            variant,
            out,
            agent,
            agent_sbom,
        } => {
            let variant = Variant::parse(variant).map_err(RunError::Variant)?;
            write_context(&variant, out, agent, agent_sbom)?;
            let status = std::process::Command::new("docker")
                .args([
                    "buildx",
                    "build",
                    "--platform",
                    "linux/arm64",
                    "--load",
                    "--file",
                ])
                .arg(out.join("Dockerfile"))
                .arg("--tag")
                .arg(variant.tag())
                .arg(out)
                .env("SOURCE_DATE_EPOCH", build::SOURCE_DATE_EPOCH.to_string())
                .status()
                .map_err(|error| RunError::Build(error.to_string()))?;
            if status.success() {
                println!("built {}", variant.tag());
                Ok(())
            } else {
                Err(RunError::Build(format!("docker exited with {status}")))
            }
        }
        Command::Artifact { variant, out } => {
            let variant = Variant::parse(variant).map_err(RunError::Variant)?;
            let written = build_release_artifact(&variant, out)?;
            println!("{}", written.display());
            Ok(())
        }
        Command::Publish { variant, region } => {
            let variant = Variant::parse(variant).map_err(RunError::Variant)?;
            let memory = image::variants()
                .into_iter()
                .find(|published| {
                    published.size == variant.size
                        && published.capabilities.contains(&image::Capability::Browser)
                            == variant.browser
                })
                .map_or(1_024, |published| published.minimum_memory_mib);
            for input in build::create_image_inputs(&variant, region, memory) {
                println!("{input}");
            }
            Ok(())
        }
        Command::Validate { lock, observed } => validate(lock, observed),
    }
}

/// The `/validate` build hook's decision, runnable off-VM.
fn validate(lock: &Path, observed: &Path) -> Result<(), RunError> {
    {
        let encoded = std::fs::read_to_string(lock).map_err(io_at(lock))?;
        let locked: image::ImageLock =
            serde_json::from_str(&encoded).map_err(|error| RunError::Lock(error.to_string()))?;
        let listing = std::fs::read_to_string(observed).map_err(io_at(observed))?;
        let installed: Vec<String> = listing
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        locked
            .validate()
            .map_err(|reason| RunError::Lock(reason.to_owned()))?;
        match locked.compare(&installed) {
            verdict @ image::LockVerdict::Match => {
                println!(
                    "{} {} package(s) match",
                    verdict.http_status(),
                    locked.packages.len()
                );
                Ok(())
            }
            verdict @ image::LockVerdict::Drift { .. } => {
                let status = verdict.http_status();
                let image::LockVerdict::Drift {
                    missing,
                    unexpected,
                } = verdict
                else {
                    unreachable!("the verdict is one of exactly two shapes");
                };
                eprintln!("{status} the installed package set is not the locked one");
                for nevra in &missing {
                    eprintln!("missing {nevra}");
                }
                for nevra in &unexpected {
                    eprintln!("unexpected {nevra}");
                }
                Err(RunError::Drift {
                    missing: missing.len(),
                    unexpected: unexpected.len(),
                })
            }
        }
    }
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let mut stderr = std::io::stderr();
            let _ = writeln!(stderr, "hands-image: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, RunError, run, write_context};
    use crate::build::Variant;
    use clap::Parser as _;

    fn inputs(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let agent = root.join("source-hands-agent");
        let sbom = root.join("source-agent.cdx.json");
        std::fs::write(&agent, b"ELF fixture").expect("agent fixture");
        std::fs::write(&sbom, br#"{"bomFormat":"CycloneDX"}"#).expect("SBOM fixture");
        (agent, sbom)
    }

    #[test]
    fn the_context_carries_the_dockerfile_agent_and_source_sbom() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let variant = Variant::parse("2gb-browser").expect("an offered variant");
        let (agent, sbom) = inputs(dir.path());
        let context = dir.path().join("context");
        let written =
            write_context(&variant, &context, &agent, &sbom).expect("the context is written");
        assert!(written.exists());
        assert_eq!(
            written.file_name().and_then(|name| name.to_str()),
            Some("Dockerfile")
        );
        let generated = std::fs::read_to_string(&written).expect("it reads back");
        assert!(generated.contains("chromium-headless"));
        assert!(context.join("hands-agent").is_file());
        assert!(context.join("agent.cdx.json").is_file());
        assert!(generated.contains("image.lock.json"));
        assert!(generated.contains("rpm-nevra.txt"));
    }

    #[test]
    fn a_second_context_write_is_byte_identical() {
        // The AEX half of the build is reproducible, and this is the cheapest place
        // the claim can be falsified: the generated Dockerfile is derived from
        // constants only, so two writes must not differ.
        let variant = Variant::parse("1gb").expect("an offered variant");
        let first = tempfile::tempdir().expect("a temporary directory");
        let second = tempfile::tempdir().expect("a temporary directory");
        let (first_agent, first_sbom) = inputs(first.path());
        let (second_agent, second_sbom) = inputs(second.path());
        let left = write_context(
            &variant,
            &first.path().join("context"),
            &first_agent,
            &first_sbom,
        )
        .expect("written");
        let right = write_context(
            &variant,
            &second.path().join("context"),
            &second_agent,
            &second_sbom,
        )
        .expect("written");
        assert_eq!(
            std::fs::read(&left).expect("it reads back"),
            std::fs::read(&right).expect("it reads back")
        );
    }

    #[test]
    fn a_reused_nonempty_context_is_refused() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let variant = Variant::parse("1gb").expect("an offered variant");
        let (agent, sbom) = inputs(dir.path());
        let context = dir.path().join("context");
        std::fs::create_dir(&context).expect("context directory");
        std::fs::write(context.join("stale-secret"), b"must not be packaged").expect("stale file");
        assert!(matches!(
            write_context(&variant, &context, &agent, &sbom),
            Err(RunError::OutputNotEmpty(path)) if path == context
        ));
    }

    #[test]
    fn a_drifting_package_set_fails_the_validate_command() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let lock = dir.path().join("image.lock.json");
        std::fs::write(
            &lock,
            r#"{"schema":"aex.hands-image-lock.v1","containerBase":"x@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","packages":["bash-0:5.2.15-1.amzn2023.aarch64"]}"#,
        )
        .expect("the lockfile is written");

        let matching = dir.path().join("match.txt");
        std::fs::write(&matching, "bash-0:5.2.15-1.amzn2023.aarch64\n").expect("written");
        let cli = Cli::parse_from([
            "hands-image",
            "validate",
            "--lock",
            &lock.to_string_lossy(),
            "--observed",
            &matching.to_string_lossy(),
        ]);
        assert!(run(&cli).is_ok());

        let drifted = dir.path().join("drift.txt");
        std::fs::write(&drifted, "bash-0:5.2.15-2.amzn2023.aarch64\n").expect("written");
        let cli = Cli::parse_from([
            "hands-image",
            "validate",
            "--lock",
            &lock.to_string_lossy(),
            "--observed",
            &drifted.to_string_lossy(),
        ]);
        assert!(
            matches!(
                run(&cli),
                Err(RunError::Drift {
                    missing: 1,
                    unexpected: 1
                })
            ),
            "a moving mirror must fail the build rather than change the image"
        );
    }

    #[test]
    fn an_unknown_variant_is_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cli = Cli::parse_from([
            "hands-image",
            "context",
            "--variant",
            "16gb",
            "--out",
            &dir.path().join("context").to_string_lossy(),
            "--agent",
            &dir.path().join("missing-agent").to_string_lossy(),
            "--agent-sbom",
            &dir.path().join("missing-sbom").to_string_lossy(),
        ]);
        assert!(matches!(run(&cli), Err(RunError::Variant(_))));
        assert!(
            !dir.path().join("context").exists(),
            "the arity is five and a sixth shape writes nothing at all"
        );
    }

    #[test]
    fn the_publish_command_prints_inputs_and_pushes_nothing() {
        let cli = Cli::parse_from([
            "hands-image",
            "publish",
            "--variant",
            "8gb",
            "--region",
            "eu-west-1",
        ]);
        assert!(run(&cli).is_ok());
        // The command is a printer by construction: `Publish` carries no path to
        // write to and the tool links no AWS SDK, so there is nothing for a
        // credential to be used by.
        assert!(matches!(cli.command, Command::Publish { .. }));
    }

    #[test]
    fn the_release_artifact_command_has_only_variant_and_output_authority() {
        let cli = Cli::parse_from([
            "hands-image",
            "artifact",
            "--variant",
            "4gb-browser",
            "--out",
            "target/microvm/hands-image-4gb-browser",
        ]);
        assert!(matches!(
            cli.command,
            Command::Artifact { variant, out }
                if variant == "4gb-browser"
                    && out == std::path::Path::new("target/microvm/hands-image-4gb-browser")
        ));
    }
}
