//! `aex-release-tool` command-line entry point.
//!
//! Every subcommand is a thin shell over the library. The only logic here is
//! argument parsing, file reading and the exit-code contract: a failure prints
//! every violation it found and exits its classification code, and nothing
//! exits `0` on a warning.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use aex_release_tool::admit::{AdmissionInputs, OperationalReadiness, Plane};
use aex_release_tool::artifact::{self, ArtifactEnvelope, Form};
use aex_release_tool::canon;
use aex_release_tool::describe;
use aex_release_tool::error::{Exit, Result, ToolError, Violation, io, usage};
use aex_release_tool::evidence::{self, DeclaredProducers, FreshnessPolicy, Receipt};
use aex_release_tool::graph::inputs::GraphInputs;
use aex_release_tool::graph::matrix::{self, MatrixKind};
use aex_release_tool::graph::select::{self, Lane, Mode};
use aex_release_tool::graph::{NodeId, verify};
use aex_release_tool::janitor::{self, Inventory, SweepMode};
use aex_release_tool::ledger::{self, JsonlLedger, LedgerEntry, LedgerStore, Readback};
use aex_release_tool::manifest::CompositionManifest;
use aex_release_tool::migration;
use aex_release_tool::oci::{
    self, OciBuildBinding, OciImageIdentity, OciSource, OciToolchain, OciWorkflowRun,
};
use aex_release_tool::policy;
use aex_release_tool::private_path;
use aex_release_tool::publication;
use aex_release_tool::release_contract;
use aex_release_tool::schemas::{self, SchemaName};
use aex_release_tool::selftest;
use aex_release_tool::verification::VerificationStatement;

/// the release graph, admission, manifest and evidence binary used by CI and deploy.
#[derive(Debug, Parser)]
#[command(
    name = "aex-release-tool",
    version,
    about = "the release graph, admission, manifest and evidence binary used by CI and deploy",
    disable_help_subcommand = true
)]
struct Cli {
    /// Repository root. Defaults to the enclosing Git work tree.
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Emit canonical JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Suppress human-readable output.
    #[arg(long, global = true)]
    quiet: bool,
    /// Accepted and ignored; output is never coloured.
    #[arg(long, global = true)]
    no_color: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build, verify, select over and explain the delivery graph.
    #[command(subcommand)]
    Graph(GraphCommand),
    /// Recipes, packaging and envelopes.
    #[command(subcommand)]
    Artifact(ArtifactCommand),
    /// Composition manifests.
    #[command(subcommand)]
    Manifest(ManifestCommand),
    /// Evidence receipts.
    #[command(subcommand)]
    Evidence(EvidenceCommand),
    /// Verification statements.
    #[command(subcommand)]
    Verification(VerificationCommand),
    /// Decide whether a candidate may be promoted.
    Admit(AdmitArgs),
    /// Terraform plan handling.
    #[command(subcommand)]
    Plan(PlanCommand),
    /// Validate public-schema/private-value release contracts.
    #[command(subcommand)]
    Contract(ContractCommand),
    /// The deployment ledger.
    #[command(subcommand)]
    Ledger(LedgerCommand),
    /// The private-path allowlist gate.
    #[command(subcommand)]
    PrivatePath(PrivatePathCommand),
    /// Migration bundles.
    #[command(subcommand)]
    Migration(MigrationCommand),
    /// Source policies over checked-in files.
    #[command(subcommand)]
    Policy(PolicyCommand),
    /// Reclaim synthetic test residue from a plane, by tag.
    #[command(subcommand)]
    Janitor(JanitorCommand),
    /// Print a release JSON Schema.
    Schema {
        /// Which schema.
        #[arg(long)]
        name: SchemaName,
    },
    /// Check the router's own determinism and monotonicity.
    Selftest,
}

#[derive(Debug, Subcommand)]
enum GraphCommand {
    /// Merge every authority into one graph and summarize it.
    Build {
        /// Write the summary here instead of to standard output.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Run every fail-closed verification rule.
    Verify {
        /// Also require executable cross-service evidence for a release route.
        #[arg(long)]
        release: bool,
    },
    /// Decide what runs.
    Select {
        /// Base commit of the diff.
        #[arg(long)]
        base: Option<String>,
        /// Head commit of the diff.
        #[arg(long)]
        head: Option<String>,
        /// Explicit changed paths, comma-separated.
        #[arg(long, value_delimiter = ',')]
        paths: Option<Vec<String>>,
        /// How wide to select.
        #[arg(long, default_value = "affected")]
        mode: Mode,
        /// Which lane is asking.
        #[arg(long, default_value = "pr")]
        lane: Lane,
        /// Write the selection here.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Explain why one node was selected.
    Explain {
        /// The node.
        #[arg(long)]
        unit: String,
        /// Base commit.
        #[arg(long)]
        base: Option<String>,
        /// Head commit.
        #[arg(long)]
        head: Option<String>,
    },
    /// Render the selection reason table.
    Diff {
        /// Base commit.
        #[arg(long)]
        base: String,
        /// Head commit.
        #[arg(long)]
        head: String,
        /// Output format.
        #[arg(long, default_value = "markdown")]
        format: DiffFormat,
    },
    /// Emit a GitHub Actions matrix.
    Matrix {
        /// The selection document.
        #[arg(long)]
        selection: PathBuf,
        /// Which slice.
        #[arg(long)]
        kind: MatrixKind,
        /// How many shards.
        #[arg(long, default_value_t = 1)]
        partitions: usize,
        /// Observed per-node durations, as canonical JSON.
        #[arg(long)]
        shard_durations: Option<PathBuf>,
        /// Append `key=value` lines here.
        #[arg(long)]
        github_output: Option<PathBuf>,
    },
}

/// How `graph diff` renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum DiffFormat {
    /// A pull-request summary table.
    Markdown,
    /// Canonical JSON.
    Json,
}

#[derive(Debug, Subcommand)]
enum ArtifactCommand {
    /// Package the committed public Terraform module closure deterministically.
    ModuleBundle {
        /// Where to write `terraform-modules.tar.gz`.
        #[arg(long)]
        out: PathBuf,
    },
    /// Validate and copy the generated regional table definition bundle.
    RegionalTables {
        /// Where to write `regional-tables.json`.
        #[arg(long)]
        out: PathBuf,
    },
    /// Print the snapshot-bound immutable tool catalogue identity.
    ToolCatalog,
    /// Print every build recipe.
    Recipes {
        /// Restrict to one unit.
        #[arg(long)]
        unit: Option<String>,
    },
    /// Print the exact build invocation. Never compiles.
    Plan {
        /// The unit.
        #[arg(long)]
        unit: String,
        /// Refuse an unbound brain-mux before any build is invoked.
        #[arg(long)]
        require_model_catalog: bool,
    },
    /// Require the complete Git worktree/index/untracked set to be clean.
    OciSourceClean {
        /// Where to write the clean-source evidence.
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify and record the exact installed OCI producer executables.
    OciToolchain {
        /// Docker Buildx executable installed by the pinned action.
        #[arg(long)]
        buildx_binary: PathBuf,
        /// Zig executable extracted from the pinned official archive.
        #[arg(long)]
        zig_binary: PathBuf,
        /// `cargo-zigbuild` executable extracted from its pinned release.
        #[arg(long)]
        cargo_zigbuild_binary: PathBuf,
        /// Where to write the closed producer identity.
        #[arg(long)]
        out: PathBuf,
    },
    /// Package a built input deterministically.
    Package {
        /// The unit.
        #[arg(long)]
        unit: String,
        /// The built binary or output tree.
        #[arg(long)]
        input: PathBuf,
        /// Where to write the archive.
        #[arg(long)]
        out: PathBuf,
        /// Override the packaged form.
        #[arg(long)]
        form: Option<Form>,
        /// Fixed archive timestamp.
        #[arg(long, default_value_t = 0)]
        source_date_epoch: u64,
    },
    /// Create a clean, source-stable OCI build context for one compiled ELF.
    OciPrepare {
        /// The OCI unit.
        #[arg(long)]
        unit: String,
        /// Authoritative recipe JSON emitted by `artifact plan`.
        #[arg(long)]
        recipe: PathBuf,
        /// Independently compiled `AArch64` ELF.
        #[arg(long)]
        input: PathBuf,
        /// New build context directory; it must not already exist.
        #[arg(long)]
        context: PathBuf,
        /// Where to write the context binding.
        #[arg(long)]
        out: PathBuf,
        /// Source `owner/repository`.
        #[arg(long)]
        repository: String,
        /// Exact source commit.
        #[arg(long)]
        commit_sha: String,
        /// Closed producer toolchain emitted by `artifact oci-toolchain`.
        #[arg(long)]
        toolchain: PathBuf,
    },
    /// Inspect one unpacked OCI layout and copy its exact manifest bytes.
    OciInspect {
        /// Unpacked OCI layout directory.
        #[arg(long)]
        layout: PathBuf,
        /// Context binding emitted by `artifact oci-prepare`.
        #[arg(long)]
        binding: PathBuf,
        /// Where to write the verified OCI identity.
        #[arg(long)]
        out: PathBuf,
        /// Where to copy the verified raw image manifest.
        #[arg(long)]
        manifest_out: PathBuf,
    },
    /// Require two independent OCI builds to have identical identities.
    OciCompare {
        /// First inspected identity.
        #[arg(long)]
        first: PathBuf,
        /// Second inspected identity.
        #[arg(long)]
        second: PathBuf,
    },
    /// Verify digest-only registry readback and bind it to this workflow run.
    OciReadback {
        /// Expected reproducible OCI identity.
        #[arg(long)]
        identity: PathBuf,
        /// Raw manifest read back from the registry.
        #[arg(long)]
        manifest: PathBuf,
        /// Config digest reported by the pulled local image.
        #[arg(long)]
        config_digest: String,
        /// ELF copied from the pulled image.
        #[arg(long)]
        binary: PathBuf,
        /// JSON emitted by `gh attestation verify --format json`.
        #[arg(long)]
        verified_provenance: PathBuf,
        /// Exact Sigstore bundle passed to the official verifier.
        #[arg(long)]
        provenance_bundle: PathBuf,
        /// Publishing workflow repository.
        #[arg(long)]
        workflow_repository: String,
        /// Publishing workflow ref.
        #[arg(long)]
        workflow_ref: String,
        /// Publishing workflow path.
        #[arg(long)]
        workflow_path: String,
        /// Publishing workflow run id.
        #[arg(long)]
        run_id: String,
        /// Publishing workflow attempt.
        #[arg(long)]
        run_attempt: u32,
        /// Matrix job name, exactly the unit id.
        #[arg(long)]
        job_name: String,
        /// OIDC builder identity for the protected workflow.
        #[arg(long)]
        builder_id: String,
        /// Where to write the publication record.
        #[arg(long)]
        out: PathBuf,
    },
    /// Decide whether GHCR visibility permits a normal or bootstrap push.
    OciVisibility {
        /// Expected reproducible OCI identity.
        #[arg(long)]
        identity: PathBuf,
        /// `missing`, `private`, or `public` from GitHub's package-read API.
        #[arg(long)]
        visibility: String,
        /// `before-push` or `after-push`.
        #[arg(long)]
        phase: String,
        /// Both explicit protected bootstrap authorities were present.
        #[arg(long)]
        bootstrap_authorized: bool,
        /// Where to retain the typed decision/blocker.
        #[arg(long)]
        out: PathBuf,
    },
    /// Assemble an envelope from a build that happened here.
    ///
    /// Every field a local build establishes is read from the tree; every field
    /// only a workflow run can establish is marked unearned and listed, so the
    /// envelope is refused by `artifact verify` rather than passing on a claim
    /// nothing backs.
    Describe {
        /// The unit.
        #[arg(long)]
        unit: String,
        /// The packaged artifact bytes.
        #[arg(long)]
        file: PathBuf,
        /// Exact recipe JSON that produced the bytes; otherwise derive it now.
        #[arg(long)]
        recipe: Option<PathBuf>,
        /// Verified OCI identity when `file` is an OCI manifest.
        #[arg(long)]
        oci_identity: Option<PathBuf>,
        /// Where to write the envelope.
        #[arg(long)]
        out: PathBuf,
        /// Where to write the ledger of fields no local build can fill.
        #[arg(long)]
        unearned_out: Option<PathBuf>,
        /// The generated contract bundle digest this build was compiled against.
        #[arg(long)]
        contract_digest: String,
        /// One argument of the argv that actually produced the bytes, repeated.
        ///
        /// Omit it when the recipe was executed as written. Supplying something
        /// else records what ran and adds a ledger row naming both, because an
        /// envelope reporting a command nobody executed is the one field in the
        /// document that cannot be checked against anything.
        #[arg(long = "ran")]
        ran: Vec<String>,
        /// Envelope creation time, RFC 3339. Defaults to now.
        #[arg(long)]
        now: Option<String>,
    },
    /// Replace a local draft's unearned fields with exact CI evidence.
    Certify {
        /// The local draft emitted by `artifact describe`.
        #[arg(long)]
        draft: PathBuf,
        /// CI/scanner claims. File-backed digests and counts are recomputed.
        #[arg(long)]
        claims: PathBuf,
        /// Packaged bytes. Omit only for an OCI registry manifest.
        #[arg(long)]
        file: Option<PathBuf>,
        /// `CycloneDX` JSON SBOM.
        #[arg(long)]
        sbom: PathBuf,
        /// Complete licence inventory from the passing scan.
        #[arg(long)]
        license_inventory: PathBuf,
        /// Complete vulnerability verdict from the passing artifact scan.
        #[arg(long)]
        vulnerability_verdict: PathBuf,
        /// Official GitHub attestation bundle.
        #[arg(long)]
        provenance_bundle: PathBuf,
        /// Verified Sigstore blob-signature bundle, required for `rust-binary`.
        #[arg(long)]
        signature_bundle: Option<PathBuf>,
        /// Passing evidence receipts, repeated.
        #[arg(long = "receipt")]
        receipts: Vec<PathBuf>,
        /// Where to write the certified envelope.
        #[arg(long)]
        out: PathBuf,
    },
    /// Record exact evidence gaps without minting a weaker envelope.
    DeferCertification {
        /// Local draft emitted by `artifact describe`.
        #[arg(long)]
        draft: PathBuf,
        /// Unearned-field ledger emitted beside the draft.
        #[arg(long)]
        unearned: PathBuf,
        /// Passing same-run receipts already bound to the artifact, repeated.
        #[arg(long = "receipt")]
        receipts: Vec<PathBuf>,
        /// Positive protected workflow run id.
        #[arg(long)]
        workflow_run_id: String,
        /// Positive protected workflow run attempt.
        #[arg(long)]
        workflow_run_attempt: u32,
        /// Artifact-builder matrix job name.
        #[arg(long)]
        job_name: String,
        /// Protected workflow builder identity.
        #[arg(long)]
        builder_id: String,
        /// Where to write the canonical deferral.
        #[arg(long)]
        out: PathBuf,
    },
    /// Exhaustively account for certified and explicitly deferred units.
    CertificationInventory {
        /// Certified envelope files, repeated.
        #[arg(long = "envelope")]
        envelopes: Vec<PathBuf>,
        /// Certification deferral files, repeated.
        #[arg(long = "deferred")]
        deferrals: Vec<PathBuf>,
        /// Exact GitHub `owner/repository`.
        #[arg(long)]
        repository: String,
        /// Exact lowercase 40-character source commit.
        #[arg(long)]
        commit_sha: String,
        /// Positive GitHub Actions run id.
        #[arg(long)]
        workflow_run_id: String,
        /// Positive GitHub Actions run attempt.
        #[arg(long)]
        workflow_run_attempt: u32,
        /// Where to write the inventory before deferred units fail the gate.
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify an envelope, and optionally the bytes it describes.
    Verify {
        /// The envelope.
        #[arg(long)]
        envelope: PathBuf,
        /// The artifact bytes.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Require a signature regardless of the unit kind's policy.
        #[arg(long)]
        require_signature: bool,
    },
    /// Print the immutable destination an envelope publishes to.
    PublishPlan {
        /// The envelope.
        #[arg(long)]
        envelope: PathBuf,
    },
    /// Derive the safe content-addressed public asset basename from real bytes.
    AssetName {
        /// Registry unit id.
        #[arg(long)]
        unit: String,
        /// Packaged artifact bytes.
        #[arg(long)]
        file: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum ManifestCommand {
    /// Assemble a complete composition from a set of envelopes.
    New {
        /// Envelope files, one per unit.
        #[arg(long = "envelope")]
        envelopes: Vec<PathBuf>,
        /// The composition inputs no envelope carries: packages, migrations,
        /// infrastructure, catalogues and the pinned policy digests.
        #[arg(long)]
        composition: PathBuf,
        /// A ledger accounting for every registry unit with no envelope. A unit
        /// that is neither described nor recorded here is a hole, and refusing
        /// is the only way it stays visible.
        #[arg(long)]
        unearned: Option<PathBuf>,
        /// Where to write the manifest.
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify the complete certified envelope set and emit the canonical
    /// composition manifest plus the artifact store admission consumes.
    Handoff {
        /// Certified envelope files, exactly one per registered deployable.
        #[arg(long = "envelope")]
        envelopes: Vec<PathBuf>,
        /// Complete non-envelope composition identities.
        #[arg(long)]
        composition: PathBuf,
        /// Where to write `composition-manifest.json`.
        #[arg(long)]
        manifest_out: PathBuf,
        /// Where to write the unit-id-to-envelope artifact store.
        #[arg(long)]
        store_out: PathBuf,
    },
    /// Derive non-envelope composition identities from source and published bytes.
    Inputs {
        /// Certified envelope files, exactly one per registered deployable.
        #[arg(long = "envelope")]
        envelopes: Vec<PathBuf>,
        /// Raw acquired release-tool executable.
        #[arg(long)]
        release_tool: PathBuf,
        /// Acquired deterministic Terraform module bundle.
        #[arg(long)]
        module_bundle: PathBuf,
        /// Acquired generated regional table bundle.
        #[arg(long)]
        regional_tables: PathBuf,
        /// Exact GitHub `owner/repository`.
        #[arg(long)]
        repository: String,
        /// Exact lowercase 40-character source commit.
        #[arg(long)]
        commit_sha: String,
        /// Positive GitHub Actions run id.
        #[arg(long)]
        workflow_run_id: String,
        /// Positive GitHub Actions run attempt.
        #[arg(long)]
        workflow_run_attempt: u64,
        /// Version reported by the acquired release tool.
        #[arg(long)]
        release_tool_version: String,
        /// Where to write canonical `composition-inputs.json`.
        #[arg(long)]
        out: PathBuf,
    },
    /// Compare two compositions.
    Diff {
        /// The older manifest.
        #[arg(long)]
        from: PathBuf,
        /// The newer manifest.
        #[arg(long)]
        to: PathBuf,
    },
    /// Recompute a manifest's digest.
    Digest {
        /// The manifest.
        #[arg(long)]
        file: PathBuf,
    },
    /// Validate a manifest.
    Validate {
        /// The manifest.
        #[arg(long)]
        file: PathBuf,
        /// Also reject environment identities, mutable references and ranges.
        #[arg(long)]
        strict_environment_scan: bool,
    },
    /// Validate a manifest and the exact downloaded public acquisition inputs.
    ValidateInputs {
        /// The manifest.
        #[arg(long)]
        file: PathBuf,
        /// Raw downloaded `aex-release-tool` executable.
        #[arg(long)]
        release_tool: PathBuf,
        /// Downloaded `terraform-modules.tar.gz`.
        #[arg(long)]
        module_bundle: PathBuf,
        /// Downloaded `regional-tables.json`.
        #[arg(long)]
        regional_tables: PathBuf,
        /// Also reject environment identities, mutable references and ranges.
        #[arg(long)]
        strict_environment_scan: bool,
    },
    /// Print the deployment order.
    Order {
        /// The manifest.
        #[arg(long)]
        file: PathBuf,
    },
    /// Which previous manifests a unit may roll back to.
    RollbackCandidates {
        /// The current manifest.
        #[arg(long)]
        file: PathBuf,
        /// The unit.
        #[arg(long)]
        unit: String,
        /// A JSON array of previous manifests.
        #[arg(long)]
        history: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum EvidenceCommand {
    /// Build a receipt from a run's context and its `JUnit` report.
    New {
        /// The run context: identity, source, selection and hygiene.
        #[arg(long)]
        context: PathBuf,
        /// The `JUnit` report the run produced.
        #[arg(long)]
        junit: PathBuf,
        /// Where to write the receipt.
        #[arg(long)]
        out: PathBuf,
    },
    /// Build a command receipt from Cargo's JSON message stream.
    NewCargo {
        /// The command context: identity, source, selection and hygiene.
        #[arg(long)]
        context: PathBuf,
        /// Cargo output produced with `--message-format=json`.
        #[arg(long)]
        messages: PathBuf,
        /// Where to write the receipt.
        #[arg(long)]
        out: PathBuf,
    },
    /// Build a receipt from a closed machine-readable check report.
    NewCheck {
        /// The check context: identity, source, selection and hygiene.
        #[arg(long)]
        context: PathBuf,
        /// Closed report emitted after the named checks completed.
        #[arg(long)]
        report: PathBuf,
        /// Where to write the receipt.
        #[arg(long)]
        out: PathBuf,
    },
    /// Hash a file and record it on a receipt.
    Attach {
        /// The receipt.
        #[arg(long)]
        receipt: PathBuf,
        /// What kind of artefact this is.
        #[arg(long)]
        kind: String,
        /// The file to hash.
        #[arg(long)]
        file: PathBuf,
        /// Where the file is stored.
        #[arg(long)]
        uri: String,
        /// Where to write the resealed receipt. Defaults to the input path.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Bind an earned receipt to a receipt-independent artifact subject and reseal it.
    BindArtifact {
        /// Sound receipt whose source and unit scope already cover the artifact.
        #[arg(long)]
        receipt: PathBuf,
        /// Draft or certified envelope carrying the artifact subject identity.
        #[arg(long)]
        envelope: PathBuf,
        /// Where to write the bound receipt. Defaults to the input receipt.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Verify one receipt.
    Verify {
        /// The receipt.
        #[arg(long)]
        receipt: PathBuf,
    },
    /// Check a receipt against its freshness class.
    Require {
        /// The receipt.
        #[arg(long)]
        receipt: PathBuf,
        /// The freshness policy.
        #[arg(long)]
        policy: PathBuf,
        /// The release the receipt must cover, where its class is release-bound.
        #[arg(long)]
        release_id: String,
        /// Evaluation time, RFC 3339. Defaults to now.
        #[arg(long)]
        now: Option<String>,
    },
    /// Compare declared jobs against collected receipts.
    Aggregate {
        /// Receipt files.
        #[arg(long = "receipt")]
        receipts: Vec<PathBuf>,
        /// The declared job list.
        #[arg(long)]
        expect: PathBuf,
        /// Where to write the lane receipt.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum VerificationCommand {
    /// Build a statement from a manifest, a binding and a receipt set.
    New {
        /// The manifest the statement is about.
        #[arg(long)]
        manifest: PathBuf,
        /// Which plane it ran against.
        #[arg(long)]
        plane: String,
        /// Digest of the environment binding that supplied the plane's values.
        #[arg(long)]
        binding_digest: String,
        /// The private-repository commit that produced the binding.
        #[arg(long)]
        binding_ref: String,
        /// Regions covered.
        #[arg(long = "region")]
        regions: Vec<String>,
        /// A JSON array of per-unit post-apply readbacks.
        #[arg(long)]
        deployed: PathBuf,
        /// Receipt files.
        #[arg(long = "receipt")]
        receipts: Vec<PathBuf>,
        /// The ledger fence this statement is anchored to.
        #[arg(long)]
        fence: u64,
        /// Where to write the statement.
        #[arg(long)]
        out: PathBuf,
        /// Statement time, RFC 3339. Defaults to now.
        #[arg(long)]
        now: Option<String>,
    },
    /// Verify a statement against a manifest.
    Verify {
        /// The statement.
        #[arg(long)]
        statement: PathBuf,
        /// The manifest.
        #[arg(long)]
        manifest: PathBuf,
    },
}

#[derive(Debug, clap::Args)]
struct AdmitArgs {
    /// The candidate manifest.
    #[arg(long)]
    manifest: PathBuf,
    /// A JSON object of unit id to envelope.
    #[arg(long)]
    envelopes: PathBuf,
    /// Receipt files.
    #[arg(long = "receipt")]
    receipts: Vec<PathBuf>,
    /// The dev verification statement.
    #[arg(long)]
    verification: Option<PathBuf>,
    /// The freshness policy.
    #[arg(long)]
    freshness: PathBuf,
    /// Which plane.
    #[arg(long)]
    plane: Plane,
    /// Workflow builder identities permitted to have built an artifact.
    #[arg(long = "builder", value_delimiter = ',')]
    builders: Vec<String>,
    /// The applied central schema head.
    #[arg(long)]
    applied_central_head: Option<String>,
    /// The applied regional generation.
    #[arg(long)]
    applied_regional_generation: Option<u32>,
    /// Evaluation time, RFC 3339.
    #[arg(long)]
    now: Option<String>,
}

#[derive(Debug, Subcommand)]
enum PlanCommand {
    /// Summarize a `terraform show -json` plan.
    Summarize {
        /// The plan document.
        #[arg(long)]
        plan_json: PathBuf,
        /// Where to write a Markdown summary.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Apply the plan policy.
    Policy {
        /// The plan document.
        #[arg(long)]
        plan_json: PathBuf,
        /// The policy.
        #[arg(long)]
        policy: PathBuf,
        /// Assert this is a manifest-only change.
        #[arg(long)]
        manifest_only: bool,
        /// Assert this is an infrastructure-only change.
        #[arg(long)]
        infrastructure_only: bool,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum ContractKind {
    /// `aex.environment-binding.v1`.
    EnvironmentBinding,
    /// `aex.resolved-placement.v1`.
    ResolvedPlacement,
    /// `aex.saved-plan-envelope.v1`.
    SavedPlanEnvelope,
}

#[derive(Debug, Subcommand)]
enum ContractCommand {
    /// Parse, close, digest-check and freshness-check a release contract.
    Validate {
        /// Contract kind.
        #[arg(long)]
        kind: ContractKind,
        /// JSON contract document.
        #[arg(long)]
        file: PathBuf,
        /// Opaque saved plan bytes. Valid only for a saved-plan envelope.
        #[arg(long)]
        payload: Option<PathBuf>,
        /// Evaluation instant, RFC 3339. Defaults to now.
        #[arg(long)]
        now: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum LedgerCommand {
    /// Append one entry.
    Append {
        /// The entry.
        #[arg(long)]
        entry: PathBuf,
        /// The JSONL store.
        #[arg(long)]
        store: PathBuf,
    },
    /// List entries for a plane.
    List {
        /// The plane.
        #[arg(long)]
        plane: String,
        /// The JSONL store.
        #[arg(long)]
        store: PathBuf,
    },
    /// Compare desired, ledger and actual.
    Verify {
        /// The plane.
        #[arg(long)]
        plane: String,
        /// The JSONL store.
        #[arg(long)]
        store: PathBuf,
        /// The desired binding digest.
        #[arg(long)]
        binding_digest: String,
        /// The observed state.
        #[arg(long)]
        actual: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum PrivatePathCommand {
    /// Classify every tracked path in a repository.
    Check {
        /// The repository to classify.
        #[arg(long)]
        target: PathBuf,
        /// The policy.
        #[arg(long)]
        policy: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum MigrationCommand {
    /// Build the bundle lock.
    Bundle {
        /// Where to write it.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Verify a bundle against the declared head.
    Verify {
        /// The bundle.
        #[arg(long)]
        bundle: PathBuf,
        /// The declared head.
        #[arg(long)]
        head: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum PolicyCommand {
    /// Scan the Terraform tree.
    Terraform,
    /// Lint every workflow.
    Workflows,
    /// Scan a Dockerfile for the COPY-only policy.
    Dockerfile {
        /// The Dockerfile.
        #[arg(long)]
        file: PathBuf,
    },
}

/// The janitor: reclamation from tags alone (OD-36).
#[derive(Debug, Subcommand)]
enum JanitorCommand {
    /// Classify, and optionally reclaim, everything a plane inventory holds.
    ///
    /// The inventory is produced by a credentialed discovery pass and is
    /// deliberately unfiltered: the guard refuses what it must not touch, and a
    /// guard only ever offered safe input is not a guard.
    Sweep {
        /// Which plane the inventory came from. Must match the document.
        #[arg(long)]
        plane: Plane,
        /// The plane inventory, as `aex.janitor-inventory.v1` JSON.
        #[arg(long)]
        inventory: PathBuf,
        /// Whether the sweep may delete. Defaults to reporting only.
        #[arg(long, default_value = "report")]
        mode: SweepMode,
        /// Evaluation instant, RFC 3339. Defaults to now.
        #[arg(long)]
        now: Option<String>,
        /// Write the sweep report here as well as to standard output.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Print the tag, TTL and reclamation-order scheme the sweep enforces.
    Scheme,
    /// Check the shipped janitor policy is internally consistent.
    Verify,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprint!("{err}");
            ExitCode::from(u8::try_from(err.exit.code()).unwrap_or(1))
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    let root = resolve_root(cli.root.as_deref())?;
    match &cli.command {
        Command::Graph(command) => run_graph(cli, &root, command),
        Command::Artifact(command) => run_artifact(cli, &root, command),
        Command::Manifest(command) => run_manifest(cli, &root, command),
        Command::Evidence(command) => run_evidence(cli, command),
        Command::Verification(command) => run_verification(cli, command),
        Command::Admit(args) => run_admit(cli, &root, args),
        Command::Plan(command) => run_plan(cli, command),
        Command::Contract(command) => run_contract(cli, command),
        Command::Ledger(command) => run_ledger(cli, command),
        Command::PrivatePath(command) => run_private_path(command),
        Command::Migration(command) => run_migration(cli, &root, command),
        Command::Policy(command) => run_policy(cli, &root, command),
        Command::Janitor(command) => run_janitor(cli, command),
        Command::Schema { name } => {
            println!("{}", schemas::text(*name).trim_end());
            Ok(())
        }
        Command::Selftest => {
            let inputs = GraphInputs::load(&root)?;
            let built = verify::build(&inputs)?;
            let report = selftest::run(&built, &inputs)?;
            emit(cli, &report)
        }
    }
}

fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = explicit {
        return Ok(root.to_path_buf());
    }
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|err| usage(format!("`git rev-parse` could not be run: {err}")))?;
    if !output.status.success() {
        return Err(usage(
            "not inside a Git work tree; pass --root to name the repository",
        ));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

fn run_graph(cli: &Cli, root: &Path, command: &GraphCommand) -> Result<()> {
    let inputs = GraphInputs::load(root)?;
    match command {
        GraphCommand::Build { out } => {
            let built = verify::build(&inputs)?;
            let summary = verify::summarize(&built);
            if let Some(path) = out {
                write_canonical(path, &summary)
            } else {
                emit(cli, &summary)
            }
        }
        GraphCommand::Verify { release } => {
            let built = if *release {
                verify::verify_release_candidate(&inputs)?
            } else {
                verify::verify(&inputs)?
            };
            emit(cli, &verify::summarize(&built))
        }
        GraphCommand::Select {
            base,
            head,
            paths,
            mode,
            lane,
            out,
        } => {
            let changed =
                resolve_changed(root, base.as_deref(), head.as_deref(), paths.as_deref())?;
            let built = verify::build(&inputs)?;
            let selection = select::select(&built, &inputs, &changed, *mode, *lane)?;
            if let Some(path) = out {
                write_canonical(path, &selection)?;
            }
            emit(cli, &selection)
        }
        GraphCommand::Explain { unit, base, head } => {
            let changed = resolve_changed(root, base.as_deref(), head.as_deref(), None)?;
            let built = verify::build(&inputs)?;
            let selection = select::select(&built, &inputs, &changed, Mode::Affected, Lane::Pr)?;
            let target = NodeId(unit.clone());
            let found = selection
                .test
                .iter()
                .chain(&selection.deploy)
                .chain(&selection.scenarios)
                .find(|selected| selected.id == target);
            match found {
                Some(selected) => {
                    println!("{}: {}", selected.id, select::describe(&selected.reason));
                    Ok(())
                }
                None => Err(ToolError::single(
                    Exit::RoutingUndecidable,
                    "graph-explain-unselected",
                    format!("`{unit}` was not selected by this change set"),
                )),
            }
        }
        GraphCommand::Diff { base, head, format } => {
            let changed = resolve_changed(root, Some(base), Some(head), None)?;
            let built = verify::build(&inputs)?;
            let selection = select::select(&built, &inputs, &changed, Mode::Affected, Lane::Pr)?;
            match format {
                DiffFormat::Markdown => {
                    print!("{}", select::to_markdown(&selection));
                    Ok(())
                }
                DiffFormat::Json => emit(cli, &selection),
            }
        }
        GraphCommand::Matrix {
            selection,
            kind,
            partitions,
            shard_durations,
            github_output,
        } => {
            let selection: select::Selection = read_json(selection)?;
            let durations: BTreeMap<String, u64> = match shard_durations {
                Some(path) => read_json(path)?,
                None => BTreeMap::new(),
            };
            let output = matrix::build(
                &selection,
                *kind,
                *partitions,
                &durations,
                &inputs.scenarios,
                &inputs.units,
                &inputs.npm,
            )?;
            if let Some(path) = github_output {
                append_text(path, &matrix::to_github_output(&output)?)?;
            }
            emit(cli, &output)
        }
    }
}

fn run_artifact(cli: &Cli, root: &Path, command: &ArtifactCommand) -> Result<()> {
    match command {
        ArtifactCommand::ModuleBundle { out } => run_module_bundle(cli, root, out),
        ArtifactCommand::RegionalTables { out } => run_regional_tables(cli, root, out),
        ArtifactCommand::ToolCatalog => run_tool_catalog(cli, root),
        ArtifactCommand::Recipes { unit } => {
            let units = read_units(root)?;
            let plans = artifact::release_recipes(&units, root)?;
            let selected: Vec<_> = plans
                .into_iter()
                .filter(|plan| unit.as_ref().is_none_or(|id| &plan.unit == id))
                .collect();
            emit(cli, &selected)
        }
        ArtifactCommand::Plan {
            unit,
            require_model_catalog,
        } => run_artifact_plan(cli, root, unit, *require_model_catalog),
        ArtifactCommand::Package {
            unit,
            input,
            out,
            form,
            source_date_epoch,
        } => {
            let units = read_units(root)?;
            let found = units
                .units
                .iter()
                .find(|candidate| &candidate.id == unit)
                .ok_or_else(|| usage(format!("`{unit}` is not in release/units.toml")))?;
            let form = match form {
                Some(form) => *form,
                None => match artifact::plan(found)?.form.as_str() {
                    "lambda-zip" => Form::LambdaZip,
                    "oci" => Form::Oci,
                    "microvm-zip" => Form::MicrovmZip,
                    "build-output" => Form::BuildOutput,
                    _ => Form::Tarball,
                },
            };
            let bytes = artifact::package(
                form,
                input,
                *source_date_epoch,
                found.entrypoint.as_deref().unwrap_or("bootstrap"),
            )?;
            std::fs::write(out, &bytes).map_err(|err| io(&out.display().to_string(), &err))?;
            emit(
                cli,
                &serde_json::json!({
                    "unit": unit,
                    "digest": canon::digest_bytes(&bytes),
                    "sizeBytes": bytes.len(),
                    "path": out.display().to_string(),
                }),
            )
        }
        command @ (ArtifactCommand::OciSourceClean { .. }
        | ArtifactCommand::OciToolchain { .. }
        | ArtifactCommand::OciPrepare { .. }
        | ArtifactCommand::OciInspect { .. }
        | ArtifactCommand::OciCompare { .. }
        | ArtifactCommand::OciReadback { .. }
        | ArtifactCommand::OciVisibility { .. }) => run_artifact_oci(cli, root, command),
        ArtifactCommand::Describe { .. } => run_artifact_describe(cli, root, command),
        ArtifactCommand::Certify { .. } => run_artifact_certify(cli, root, command),
        ArtifactCommand::DeferCertification { .. } => {
            run_artifact_defer_certification(cli, root, command)
        }
        ArtifactCommand::CertificationInventory { .. } => {
            run_artifact_certification_inventory(cli, root, command)
        }
        ArtifactCommand::Verify {
            envelope,
            file,
            require_signature,
        } => {
            let envelope: ArtifactEnvelope = read_json(envelope)?;
            envelope.verify(file.as_deref(), *require_signature)?;
            emit(
                cli,
                &serde_json::json!({ "unit": envelope.unit.id, "verified": true }),
            )
        }
        ArtifactCommand::PublishPlan { envelope } => {
            let envelope: ArtifactEnvelope = read_json(envelope)?;
            emit(cli, &artifact::publish_destination(&envelope)?)
        }
        ArtifactCommand::AssetName { .. } => run_artifact_asset_name(cli, root, command),
    }
}

fn run_tool_catalog(cli: &Cli, root: &Path) -> Result<()> {
    let digest = artifact::tool_catalog_digest(root)?;
    emit(
        cli,
        &serde_json::json!({
            "schema": "aex.tool-catalog-identity.v1",
            "digest": digest,
            "source": "crates/aex-brain-tool-catalog/src/catalog.rs",
        }),
    )
}

fn run_artifact_describe(cli: &Cli, root: &Path, command: &ArtifactCommand) -> Result<()> {
    let ArtifactCommand::Describe {
        unit,
        file,
        recipe,
        oci_identity,
        out,
        unearned_out,
        contract_digest,
        ran,
        now,
    } = command
    else {
        return Err(usage("internal artifact description dispatch mismatch"));
    };
    run_describe(
        cli,
        root,
        unit,
        file,
        recipe.as_deref(),
        oci_identity.as_deref(),
        out,
        unearned_out.as_deref(),
        contract_digest,
        ran,
        now.as_deref(),
    )
}

fn run_artifact_oci(cli: &Cli, root: &Path, command: &ArtifactCommand) -> Result<()> {
    match command {
        ArtifactCommand::OciSourceClean { out } => {
            let evidence = oci::verify_clean(root)?;
            write_canonical(out, &evidence)?;
            emit(cli, &evidence)
        }
        ArtifactCommand::OciToolchain {
            buildx_binary,
            zig_binary,
            cargo_zigbuild_binary,
            out,
        } => {
            let toolchain =
                oci::inspect_toolchain(buildx_binary, zig_binary, cargo_zigbuild_binary)?;
            write_canonical(out, &toolchain)?;
            emit(cli, &toolchain)
        }
        ArtifactCommand::OciPrepare {
            unit,
            recipe,
            input,
            context,
            out,
            repository,
            commit_sha,
            toolchain,
        } => {
            let units = read_units(root)?;
            let found = units
                .units
                .iter()
                .find(|candidate| &candidate.id == unit)
                .ok_or_else(|| usage(format!("`{unit}` is not in release/units.toml")))?;
            let recipe: artifact::BuildPlan = read_json(recipe)?;
            let toolchain: OciToolchain = read_json(toolchain)?;
            let binding = oci::prepare_context(
                found,
                &recipe,
                input,
                context,
                OciSource {
                    repository: repository.clone(),
                    commit_sha: commit_sha.clone(),
                },
                toolchain,
            )?;
            write_canonical(out, &binding)?;
            emit(cli, &binding)
        }
        ArtifactCommand::OciInspect {
            layout,
            binding,
            out,
            manifest_out,
        } => {
            let binding: OciBuildBinding = read_json(binding)?;
            let identity = oci::inspect_layout(layout, &binding)?;
            let source = oci::manifest_blob_path(layout, &identity);
            let bytes =
                std::fs::read(&source).map_err(|err| io(&source.display().to_string(), &err))?;
            std::fs::write(manifest_out, bytes)
                .map_err(|err| io(&manifest_out.display().to_string(), &err))?;
            write_canonical(out, &identity)?;
            emit(cli, &identity)
        }
        ArtifactCommand::OciCompare { first, second } => {
            let first: OciImageIdentity = read_json(first)?;
            let second: OciImageIdentity = read_json(second)?;
            oci::verify_reproducible(&first, &second)?;
            emit(
                cli,
                &serde_json::json!({
                    "unit": first.unit,
                    "digest": first.output_digest,
                    "reproducible": true,
                }),
            )
        }
        command @ ArtifactCommand::OciReadback { .. } => run_oci_readback(cli, command),
        command @ ArtifactCommand::OciVisibility { .. } => run_oci_visibility(cli, command),
        _ => Err(usage("internal OCI artifact dispatch mismatch")),
    }
}

fn run_oci_readback(cli: &Cli, command: &ArtifactCommand) -> Result<()> {
    let ArtifactCommand::OciReadback {
        identity,
        manifest,
        config_digest,
        binary,
        verified_provenance,
        provenance_bundle,
        workflow_repository,
        workflow_ref,
        workflow_path,
        run_id,
        run_attempt,
        job_name,
        builder_id,
        out,
    } = command
    else {
        return Err(usage("internal OCI readback dispatch mismatch"));
    };
    let identity: OciImageIdentity = read_json(identity)?;
    let publication = oci::verify_readback(
        &identity,
        manifest,
        config_digest,
        binary,
        OciWorkflowRun {
            repository: workflow_repository.clone(),
            r#ref: workflow_ref.clone(),
            path: workflow_path.clone(),
            run_id: run_id.clone(),
            run_attempt: *run_attempt,
            job_name: job_name.clone(),
            builder_id: builder_id.clone(),
        },
        verified_provenance,
        provenance_bundle,
    )?;
    write_canonical(out, &publication)?;
    emit(cli, &publication)
}

fn run_oci_visibility(cli: &Cli, command: &ArtifactCommand) -> Result<()> {
    let ArtifactCommand::OciVisibility {
        identity,
        visibility,
        phase,
        bootstrap_authorized,
        out,
    } = command
    else {
        return Err(usage("internal OCI visibility dispatch mismatch"));
    };
    let identity: OciImageIdentity = read_json(identity)?;
    let visibility = visibility.parse().map_err(usage)?;
    let phase = phase.parse().map_err(usage)?;
    let decision = oci::decide_visibility(&identity, visibility, phase, *bootstrap_authorized);
    write_canonical(out, &decision)?;
    emit(cli, &decision)?;
    if let Some(error) = decision.blocked_error() {
        Err(error)
    } else {
        Ok(())
    }
}

fn run_artifact_certify(cli: &Cli, root: &Path, command: &ArtifactCommand) -> Result<()> {
    let ArtifactCommand::Certify {
        draft,
        claims,
        file,
        sbom,
        license_inventory,
        vulnerability_verdict,
        provenance_bundle,
        signature_bundle,
        receipts,
        out,
    } = command
    else {
        return Err(usage("internal artifact certification dispatch mismatch"));
    };
    let draft: ArtifactEnvelope = read_json(draft)?;
    let units = read_units(root)?;
    let unit = units
        .units
        .iter()
        .find(|candidate| candidate.id == draft.unit.id)
        .ok_or_else(|| usage(format!("`{}` is not in release/units.toml", draft.unit.id)))?;
    let claims: aex_release_tool::certify::CertificationClaims = read_json(claims)?;
    let receipts = receipts
        .iter()
        .map(|path| read_json(path))
        .collect::<Result<Vec<_>>>()?;
    let freshness: FreshnessPolicy = read_toml(&root.join("release/policy/freshness.toml"))?;
    let envelope = aex_release_tool::certify::certify(
        draft,
        unit,
        claims,
        aex_release_tool::certify::CertificationFiles {
            artifact: file.as_deref(),
            sbom,
            license_inventory,
            vulnerability_verdict,
            provenance_bundle,
            signature_bundle: signature_bundle.as_deref(),
        },
        &receipts,
        &freshness,
    )?;
    write_canonical(out, &envelope)?;
    emit(
        cli,
        &serde_json::json!({
            "unit": envelope.unit.id,
            "envelopeDigest": envelope.envelope_digest,
            "artifactSubjectDigest": envelope.artifact_subject_digest,
            "artifactDigest": envelope.output.digest,
            "location": envelope.output.location,
        }),
    )
}

fn run_artifact_defer_certification(
    cli: &Cli,
    root: &Path,
    command: &ArtifactCommand,
) -> Result<()> {
    let ArtifactCommand::DeferCertification {
        draft,
        unearned,
        receipts,
        workflow_run_id,
        workflow_run_attempt,
        job_name,
        builder_id,
        out,
    } = command
    else {
        return Err(usage(
            "internal artifact certification-deferral dispatch mismatch",
        ));
    };
    let draft: ArtifactEnvelope = read_json(draft)?;
    let units = read_units(root)?;
    let unit = units
        .units
        .iter()
        .find(|candidate| candidate.id == draft.unit.id)
        .ok_or_else(|| usage(format!("`{}` is not in release/units.toml", draft.unit.id)))?;
    let unearned = read_json(unearned)?;
    let receipts = receipts
        .iter()
        .map(|path| read_json(path))
        .collect::<Result<Vec<_>>>()?;
    let freshness: FreshnessPolicy = read_toml(&root.join("release/policy/freshness.toml"))?;
    let workflow = aex_release_tool::artifact::Workflow {
        repository: draft.source.repository.clone(),
        r#ref: "refs/heads/main".to_owned(),
        path: ".github/workflows/_build-artifacts.yml".to_owned(),
        run_id: workflow_run_id.clone(),
        run_attempt: *workflow_run_attempt,
        job_name: job_name.clone(),
        builder_id: builder_id.clone(),
    };
    let deferral = aex_release_tool::certification::defer(
        &draft, unit, workflow, unearned, &receipts, &freshness,
    )?;
    write_canonical(out, &deferral)?;
    emit(cli, &deferral)
}

fn run_artifact_certification_inventory(
    cli: &Cli,
    root: &Path,
    command: &ArtifactCommand,
) -> Result<()> {
    let ArtifactCommand::CertificationInventory {
        envelopes,
        deferrals,
        repository,
        commit_sha,
        workflow_run_id,
        workflow_run_attempt,
        out,
    } = command
    else {
        return Err(usage(
            "internal artifact certification-inventory dispatch mismatch",
        ));
    };
    let registry = read_units(root)?;
    let envelopes = envelopes
        .iter()
        .map(|path| read_json(path))
        .collect::<Result<Vec<_>>>()?;
    let deferrals = deferrals
        .iter()
        .map(|path| read_json(path))
        .collect::<Result<Vec<_>>>()?;
    let report = aex_release_tool::certification::inventory(
        &registry,
        &envelopes,
        &deferrals,
        &aex_release_tool::certification::ExpectedSource {
            repository,
            commit_sha,
            run_id: workflow_run_id,
            run_attempt: *workflow_run_attempt,
        },
    )?;
    write_canonical(out, &report)?;
    emit(cli, &report)?;
    if let Some(error) = report.blocking_error() {
        Err(error)
    } else {
        Ok(())
    }
}

fn run_artifact_asset_name(cli: &Cli, root: &Path, command: &ArtifactCommand) -> Result<()> {
    let ArtifactCommand::AssetName { unit, file } = command else {
        return Err(usage("internal artifact asset-name dispatch mismatch"));
    };
    let units = read_units(root)?;
    let found = units
        .units
        .iter()
        .find(|candidate| candidate.id == *unit)
        .ok_or_else(|| usage(format!("`{unit}` is not in release/units.toml")))?;
    if found.kind.starts_with("rust-oci-") {
        return Err(usage(format!(
            "unit `{unit}` is OCI and must publish a registry manifest, not a release asset"
        )));
    }
    let bytes = std::fs::read(file).map_err(|err| io(&file.display().to_string(), &err))?;
    let digest = canon::digest_bytes(&bytes);
    let asset = publication::unit_asset_name(unit, &digest, &found.form)?;
    emit(
        cli,
        &serde_json::json!({
            "unit": unit,
            "asset": asset,
            "digest": digest,
            "sizeBytes": bytes.len(),
        }),
    )
}

fn run_module_bundle(cli: &Cli, root: &Path, out: &Path) -> Result<()> {
    let bytes = publication::package_module_bundle(root)?;
    std::fs::write(out, &bytes).map_err(|err| io(&out.display().to_string(), &err))?;
    emit(
        cli,
        &serde_json::json!({
            "asset": publication::MODULE_BUNDLE_ASSET,
            "digest": canon::digest_bytes(&bytes),
            "sizeBytes": bytes.len(),
            "path": out.display().to_string(),
        }),
    )
}

fn run_regional_tables(cli: &Cli, root: &Path, out: &Path) -> Result<()> {
    let (bytes, identity) = publication::regional_tables_bundle(root)?;
    std::fs::write(out, &bytes).map_err(|err| io(&out.display().to_string(), &err))?;
    emit(
        cli,
        &serde_json::json!({
            "asset": publication::REGIONAL_TABLES_ASSET,
            "digest": identity.digest,
            "sizeBytes": identity.size_bytes,
            "definitionsDigest": identity.definitions_digest,
            "path": out.display().to_string(),
        }),
    )
}

fn run_artifact_plan(
    cli: &Cli,
    root: &Path,
    unit: &str,
    require_model_catalog: bool,
) -> Result<()> {
    let units = read_units(root)?;
    let found = units
        .units
        .iter()
        .find(|candidate| candidate.id == unit)
        .ok_or_else(|| usage(format!("`{unit}` is not in release/units.toml")))?;
    let plan = if require_model_catalog {
        artifact::publication_plan(found, root)?
    } else {
        artifact::release_plan(found, root)?
    };
    emit(cli, &plan)
}

/// Assemble an envelope for a unit built on this machine.
///
/// Everything the tree can establish is read from it; the ledger the call
/// returns names every field only a workflow run can fill.
#[allow(clippy::too_many_arguments)]
fn run_describe(
    cli: &Cli,
    root: &Path,
    unit: &str,
    file: &Path,
    recipe: Option<&Path>,
    oci_identity: Option<&Path>,
    out: &Path,
    unearned_out: Option<&Path>,
    contract_digest: &str,
    ran: &[String],
    now: Option<&str>,
) -> Result<()> {
    let units = read_units(root)?;
    let found = units
        .units
        .iter()
        .find(|candidate| candidate.id == unit)
        .ok_or_else(|| usage(format!("`{unit}` is not in release/units.toml")))?;
    let plan = if let Some(path) = recipe {
        read_json(path)?
    } else {
        artifact::release_plan(found, root)?
    };
    let inputs = GraphInputs::load(root)?;
    let built = verify::build(&inputs)?;
    let oci_identity: Option<OciImageIdentity> = oci_identity.map(read_json).transpose()?;
    let mut toolchain = local_toolchain(&found.target)?;
    if let Some(image) = &oci_identity {
        image.toolchain.enrich_envelope(&mut toolchain);
    }
    let local = describe::LocalBuild {
        unit: found,
        plan: &plan,
        artifact: file,
        oci_identity: oci_identity.as_ref(),
        repository: "aexhq/aex".to_owned(),
        commit_sha: git_output(root, &["rev-parse", "HEAD"])?,
        tree_clean: git_output(root, &["status", "--porcelain"])?.is_empty(),
        git_ref: source_ref(root)?,
        toolchain,
        lockfile_digest: file_digest(&root.join(lockfile_for(&found.kind)))?,
        contract_digest: contract_digest.to_owned(),
        actual_argv: (!ran.is_empty()).then(|| ran.to_vec()),
        closure: input_closure(root, &inputs, &built, unit)?,
        location_uri: relative_to(root, file),
        receipts: Vec::new(),
        created_at: parse_now(now)?
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|err| usage(format!("cannot format the timestamp: {err}")))?,
    };
    let (envelope, unearned) = describe::describe(&local)?;
    write_canonical(out, &envelope)?;
    if let Some(path) = unearned_out {
        write_canonical(path, &unearned)?;
    }
    emit(
        cli,
        &serde_json::json!({
            "unit": unit,
            "envelopeDigest": envelope.envelope_digest,
            "artifactDigest": envelope.output.digest,
            "sizeBytes": envelope.output.size_bytes,
            "inputClosureCount": envelope.inputs.input_closure_count,
            "unearned": unearned,
        }),
    )
}

/// Assemble a complete composition, refusing any deployable that is neither
/// described nor recorded as unearned.
fn run_manifest_new(
    cli: &Cli,
    root: &Path,
    envelopes: &[PathBuf],
    composition: &Path,
    unearned: Option<&Path>,
    out: &Path,
) -> Result<()> {
    let described: BTreeMap<String, ArtifactEnvelope> = envelopes
        .iter()
        .map(|path| {
            let envelope: ArtifactEnvelope = read_json(path)?;
            Ok((envelope.unit.id.clone(), envelope))
        })
        .collect::<Result<_>>()?;
    let inputs: aex_release_tool::manifest::CompositionInputs = read_json(composition)?;
    let recorded: Vec<aex_workspace_check::registry::UnearnedRow> = match unearned {
        Some(path) => read_json(path)?,
        None => Vec::new(),
    };
    let registry = read_units(root)?;
    let holes = aex_release_tool::manifest::unaccounted_units(&registry, &described, &recorded);
    if !holes.is_empty() {
        return Err(ToolError::many(Exit::CompositionIncompatible, holes));
    }
    let manifest = aex_release_tool::manifest::new_manifest(inputs, &described, &registry)?;
    write_canonical(out, &manifest)?;
    emit(
        cli,
        &serde_json::json!({
            "releaseId": manifest.release_id,
            "units": manifest.units.len(),
            "stages": manifest.order.len(),
            "unearned": recorded.iter().map(|row| &row.subject).collect::<Vec<_>>(),
        }),
    )
}

/// Produce the two immutable public handoff documents only after every
/// envelope and every cross-document identity verifies.
fn run_manifest_handoff(
    cli: &Cli,
    root: &Path,
    envelope_paths: &[PathBuf],
    composition: &Path,
    manifest_out: &Path,
    store_out: &Path,
) -> Result<()> {
    if manifest_out == store_out {
        return Err(usage(
            "`--manifest-out` and `--store-out` must name different files",
        ));
    }
    let envelopes = envelope_paths
        .iter()
        .map(|path| read_json(path))
        .collect::<Result<Vec<ArtifactEnvelope>>>()?;
    let registry = read_units(root)?;
    // Audit the envelopes first. This intentionally reports the concrete
    // certification/receipt gaps even while the separate composition-input
    // producer is still absent.
    let store = aex_release_tool::manifest::verify_handoff_envelopes(&registry, envelopes)?;
    if !composition.is_file() {
        return Err(ToolError::single(
            Exit::EvidenceMissing,
            "handoff-composition-inputs-missing",
            format!(
                "complete composition inputs were not published at `{}`",
                composition.display()
            ),
        ));
    }
    let inputs: aex_release_tool::manifest::CompositionInputs = read_json(composition)?;
    let manifest = aex_release_tool::manifest::new_handoff_manifest(inputs, &store, &registry)?;
    let manifest_digest = canon::digest_bytes(&canon::to_file_bytes(&manifest)?);
    let store_digest = canon::digest_bytes(&canon::to_file_bytes(&store)?);

    // Nothing is written before the complete store and strict manifest have
    // both passed. Admission consumes the store as this exact canonical map.
    write_canonical(manifest_out, &manifest)?;
    write_canonical(store_out, &store)?;
    emit(
        cli,
        &serde_json::json!({
            "releaseId": manifest.release_id,
            "manifestDigest": manifest_digest,
            "artifactStoreDigest": store_digest,
            "units": store.len(),
            "stages": manifest.order.len(),
        }),
    )
}

fn run_manifest_inputs(
    cli: &Cli,
    root: &Path,
    envelope_paths: &[PathBuf],
    files: aex_release_tool::composition_inputs::PublicInputFiles<'_>,
    run: &aex_release_tool::composition_inputs::PublicRunIdentity,
    out: &Path,
) -> Result<()> {
    let envelopes = envelope_paths
        .iter()
        .map(|path| read_json(path))
        .collect::<Result<Vec<ArtifactEnvelope>>>()?;
    let registry = read_units(root)?;
    let (_, authorities) =
        aex_release_tool::composition_inputs::envelope_authorities(&registry, envelopes)?;
    let inputs =
        aex_release_tool::composition_inputs::produce(root, &registry, files, run, authorities)?;
    let digest = canon::digest_bytes(&canon::to_file_bytes(&inputs)?);
    write_canonical(out, &inputs)?;
    emit(
        cli,
        &serde_json::json!({
            "schema": "aex.composition-inputs.v1",
            "digest": digest,
            "packages": inputs.packages.len(),
            "catalogs": inputs.catalogs.len(),
            "providers": inputs.infra.provider_versions.len(),
            "path": out.display().to_string(),
        }),
    )
}

fn run_manifest_inputs_command(cli: &Cli, root: &Path, command: &ManifestCommand) -> Result<()> {
    let ManifestCommand::Inputs {
        envelopes,
        release_tool,
        module_bundle,
        regional_tables,
        repository,
        commit_sha,
        workflow_run_id,
        workflow_run_attempt,
        release_tool_version,
        out,
    } = command
    else {
        unreachable!("manifest inputs command arm only");
    };
    let run = aex_release_tool::composition_inputs::PublicRunIdentity {
        repository: repository.clone(),
        commit_sha: commit_sha.clone(),
        workflow_run_id: workflow_run_id.clone(),
        workflow_run_attempt: *workflow_run_attempt,
        release_tool_version: release_tool_version.clone(),
    };
    run_manifest_inputs(
        cli,
        root,
        envelopes,
        aex_release_tool::composition_inputs::PublicInputFiles {
            release_tool,
            module_bundle,
            regional_tables,
        },
        &run,
        out,
    )
}

fn run_manifest(cli: &Cli, root: &Path, command: &ManifestCommand) -> Result<()> {
    match command {
        ManifestCommand::New {
            envelopes,
            composition,
            unearned,
            out,
        } => run_manifest_new(cli, root, envelopes, composition, unearned.as_deref(), out),
        ManifestCommand::Handoff {
            envelopes,
            composition,
            manifest_out,
            store_out,
        } => run_manifest_handoff(cli, root, envelopes, composition, manifest_out, store_out),
        ManifestCommand::Inputs { .. } => run_manifest_inputs_command(cli, root, command),
        ManifestCommand::Diff { from, to } => {
            let from: CompositionManifest = read_json(from)?;
            let to: CompositionManifest = read_json(to)?;
            emit(cli, &aex_release_tool::manifest::diff(&from, &to))
        }
        ManifestCommand::Digest { file } => {
            let manifest: CompositionManifest = read_json(file)?;
            println!("{}", manifest.digest()?);
            Ok(())
        }
        ManifestCommand::Validate {
            file,
            strict_environment_scan,
        } => {
            let manifest: CompositionManifest = read_json(file)?;
            manifest.validate(*strict_environment_scan)?;
            emit(
                cli,
                &serde_json::json!({ "releaseId": manifest.release_id, "valid": true }),
            )
        }
        ManifestCommand::ValidateInputs {
            file,
            release_tool,
            module_bundle,
            regional_tables,
            strict_environment_scan,
        } => {
            let manifest: CompositionManifest = read_json(file)?;
            manifest.validate_acquired_inputs(
                release_tool,
                module_bundle,
                regional_tables,
                *strict_environment_scan,
            )?;
            emit(
                cli,
                &serde_json::json!({ "releaseId": manifest.release_id, "inputsValid": true }),
            )
        }
        ManifestCommand::Order { file } => {
            let manifest: CompositionManifest = read_json(file)?;
            emit(cli, &manifest.order)
        }
        ManifestCommand::RollbackCandidates {
            file,
            unit,
            history,
        } => {
            let manifest: CompositionManifest = read_json(file)?;
            let history: Vec<CompositionManifest> = read_json(history)?;
            let candidates =
                aex_release_tool::manifest::rollback_candidates(&history, &manifest, unit);
            emit(
                cli,
                &serde_json::json!({ "unit": unit, "candidates": candidates }),
            )
        }
    }
}

#[allow(clippy::too_many_lines)]
fn run_evidence(cli: &Cli, command: &EvidenceCommand) -> Result<()> {
    match command {
        EvidenceCommand::New {
            context,
            junit,
            out,
        } => {
            let context: evidence::RunContext = read_json(context)?;
            let xml = std::fs::read_to_string(junit)
                .map_err(|err| io(&junit.display().to_string(), &err))?;
            let receipt = evidence::new_receipt(context, &evidence::parse_junit(&xml))?;
            write_canonical(out, &receipt)?;
            emit(
                cli,
                &serde_json::json!({
                    "receiptId": receipt.receipt_id,
                    "receiptDigest": receipt.receipt_digest,
                    "conclusion": receipt.conclusion,
                    "inventory": receipt.inventory,
                }),
            )
        }
        EvidenceCommand::NewCargo {
            context,
            messages,
            out,
        } => {
            let context: evidence::RunContext = read_json(context)?;
            let messages = std::fs::read_to_string(messages)
                .map_err(|err| io(&messages.display().to_string(), &err))?;
            let summary = evidence::parse_cargo_messages(&messages)?;
            let receipt = evidence::new_command_receipt(context, summary)?;
            write_canonical(out, &receipt)?;
            emit(
                cli,
                &serde_json::json!({
                    "receiptId": receipt.receipt_id,
                    "receiptDigest": receipt.receipt_digest,
                    "conclusion": receipt.conclusion,
                    "inventory": receipt.inventory,
                }),
            )
        }
        EvidenceCommand::NewCheck {
            context,
            report,
            out,
        } => {
            let context: evidence::RunContext = read_json(context)?;
            let report: evidence::CheckReport = read_json(report)?;
            let receipt = evidence::new_check_receipt(context, &report)?;
            write_canonical(out, &receipt)?;
            emit(
                cli,
                &serde_json::json!({
                    "receiptId": receipt.receipt_id,
                    "receiptDigest": receipt.receipt_digest,
                    "conclusion": receipt.conclusion,
                    "inventory": receipt.inventory,
                }),
            )
        }
        EvidenceCommand::Attach {
            receipt,
            kind,
            file,
            uri,
            out,
        } => run_evidence_attach(cli, receipt, kind, file, uri, out.as_ref()),
        EvidenceCommand::BindArtifact {
            receipt,
            envelope,
            out,
        } => run_evidence_bind_artifact(cli, receipt, envelope, out.as_ref()),
        EvidenceCommand::Verify { receipt } => {
            let receipt: Receipt = read_json(receipt)?;
            receipt.verify()?;
            emit(
                cli,
                &serde_json::json!({ "receiptId": receipt.receipt_id, "verified": true }),
            )
        }
        EvidenceCommand::Require {
            receipt,
            policy,
            release_id,
            now,
        } => {
            let receipt: Receipt = read_json(receipt)?;
            let policy: FreshnessPolicy = read_toml(policy)?;
            evidence::check_freshness(&receipt, &policy, release_id, parse_now(now.as_deref())?)?;
            emit(
                cli,
                &serde_json::json!({ "receiptId": receipt.receipt_id, "fresh": true }),
            )
        }
        EvidenceCommand::Aggregate {
            receipts,
            expect,
            out,
        } => {
            let receipts: Vec<Receipt> = receipts
                .iter()
                .map(|path| read_json(path))
                .collect::<Result<_>>()?;
            let declared: DeclaredProducers = read_json(expect)?;
            let lane = evidence::aggregate(&receipts, &declared)?;
            if let Some(path) = out {
                write_canonical(path, &lane)?;
            }
            emit(cli, &lane)
        }
    }
}

fn run_evidence_bind_artifact(
    cli: &Cli,
    receipt: &Path,
    envelope: &Path,
    out: Option<&PathBuf>,
) -> Result<()> {
    let loaded: Receipt = read_json(receipt)?;
    let original_receipt_digest = loaded.receipt_digest.clone();
    let envelope: ArtifactEnvelope = read_json(envelope)?;
    let bound = evidence::bind_artifact(loaded, &envelope)?;
    write_canonical(out.map_or(receipt, PathBuf::as_path), &bound)?;
    emit(
        cli,
        &serde_json::json!({
            "receiptId": bound.receipt_id,
            "originalReceiptDigest": original_receipt_digest,
            "receiptDigest": bound.receipt_digest,
            "artifactSubjectDigest": bound.subject.artifact_subject_digest,
        }),
    )
}

fn run_evidence_attach(
    cli: &Cli,
    receipt: &Path,
    kind: &str,
    file: &Path,
    uri: &str,
    out: Option<&PathBuf>,
) -> Result<()> {
    let loaded: Receipt = read_json(receipt)?;
    let attached = evidence::attach(loaded, kind, file, uri)?;
    write_canonical(out.map_or(receipt, PathBuf::as_path), &attached)?;
    emit(
        cli,
        &serde_json::json!({
            "receiptId": attached.receipt_id,
            "receiptDigest": attached.receipt_digest,
            "attachments": attached.attachments,
        }),
    )
}

fn run_verification(cli: &Cli, command: &VerificationCommand) -> Result<()> {
    match command {
        VerificationCommand::New {
            manifest,
            plane,
            binding_digest,
            binding_ref,
            regions,
            deployed,
            receipts,
            fence,
            out,
            now,
        } => {
            let manifest: CompositionManifest = read_json(manifest)?;
            let deployed: Vec<aex_release_tool::verification::Deployed> = read_json(deployed)?;
            let receipts: Vec<Receipt> = receipts
                .iter()
                .map(|path| read_json(path))
                .collect::<Result<_>>()?;
            let now = parse_now(now.as_deref())?
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|err| usage(format!("cannot format the timestamp: {err}")))?;
            let statement = aex_release_tool::verification::new_statement(
                &manifest,
                plane,
                binding_digest,
                binding_ref,
                regions.clone(),
                deployed,
                &receipts,
                *fence,
                &now,
            )?;
            write_canonical(out, &statement)?;
            emit(
                cli,
                &serde_json::json!({
                    "releaseId": statement.release_id,
                    "statementDigest": statement.statement_digest,
                    "conclusion": statement.conclusion,
                    "deployed": statement.deployed.len(),
                }),
            )
        }
        VerificationCommand::Verify {
            statement,
            manifest,
        } => {
            let statement: VerificationStatement = read_json(statement)?;
            let manifest: CompositionManifest = read_json(manifest)?;
            statement.verify(&manifest)
        }
    }
}

fn run_admit(cli: &Cli, root: &Path, args: &AdmitArgs) -> Result<()> {
    let manifest: CompositionManifest = read_json(&args.manifest)?;
    let envelopes: BTreeMap<String, ArtifactEnvelope> = read_json(&args.envelopes)?;
    let receipts: Vec<Receipt> = args
        .receipts
        .iter()
        .map(|path| read_json(path))
        .collect::<Result<_>>()?;
    let statement: Option<VerificationStatement> = match &args.verification {
        Some(path) => Some(read_json(path)?),
        None => None,
    };
    let freshness: FreshnessPolicy = read_toml(&args.freshness)?;
    let required = required_receipts_by_kind(&manifest);
    // The ledger of evidence that cannot be earned yet is a repository fact,
    // not an argument: admission reads it so a refusal can name the stream
    // that owes the missing receipt.
    let unearned = aex_release_tool::test_registry::UnearnedIndex::load(root)?;
    let admission = aex_release_tool::admit::admit(&AdmissionInputs {
        manifest: &manifest,
        envelopes: &envelopes,
        receipts: &receipts,
        statement: statement.as_ref(),
        freshness: &freshness,
        plane: args.plane,
        // Nothing here reads a plane. Operational readiness arrives as an
        // input so `admit` stays credential-free; the release lane supplies it
        // from the scheduled `plane` suite's receipts.
        readiness: OperationalReadiness::default(),
        required_receipts: &required,
        builder_allowlist: &args.builders,
        applied_central_head: args.applied_central_head.clone(),
        applied_regional_generation: args.applied_regional_generation,
        unearned: &unearned,
        now: parse_now(args.now.as_deref())?,
    })?;
    emit(cli, &admission)
}

fn required_receipts_by_kind(manifest: &CompositionManifest) -> BTreeMap<String, Vec<String>> {
    let mut required: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in manifest.units.values() {
        required.entry(entry.kind.clone()).or_insert_with(|| {
            vec![
                "unit".to_owned(),
                "lint".to_owned(),
                "sbom".to_owned(),
                "license".to_owned(),
                "vulnerability".to_owned(),
            ]
        });
    }
    required
}

fn run_plan(cli: &Cli, command: &PlanCommand) -> Result<()> {
    match command {
        PlanCommand::Summarize { plan_json, out } => {
            let plan: serde_json::Value = read_json(plan_json)?;
            let summary = policy::summarize_plan(&plan)?;
            if let Some(path) = out {
                let markdown = format!(
                    "### Terraform plan\n\n- create: {}\n- update: {}\n- replace: {}\n- delete: {}\n",
                    summary.create.len(),
                    summary.update.len(),
                    summary.replace.len(),
                    summary.delete.len()
                );
                std::fs::write(path, markdown)
                    .map_err(|err| io(&path.display().to_string(), &err))?;
            }
            emit(cli, &summary)
        }
        PlanCommand::Policy {
            plan_json,
            policy: policy_path,
            manifest_only,
            infrastructure_only,
        } => {
            if *manifest_only && *infrastructure_only {
                return Err(usage(
                    "--manifest-only and --infrastructure-only are mutually exclusive",
                ));
            }
            let plan: serde_json::Value = read_json(plan_json)?;
            let policy_doc: policy::PlanPolicy = read_toml(policy_path)?;
            let summary = policy::summarize_plan(&plan)?;
            policy::check_plan(&summary, &policy_doc)?;
            if *manifest_only || *infrastructure_only {
                policy::check_change_isolation(&summary, *manifest_only)?;
            }
            emit(cli, &summary)
        }
    }
}

fn run_contract(cli: &Cli, command: &ContractCommand) -> Result<()> {
    match command {
        ContractCommand::Validate {
            kind,
            file,
            payload,
            now,
        } => {
            let text = std::fs::read_to_string(file)
                .map_err(|err| io(&file.display().to_string(), &err))?;
            match kind {
                ContractKind::EnvironmentBinding => {
                    if payload.is_some() || now.is_some() {
                        return Err(usage(
                            "--payload and --now are valid only for saved-plan-envelope",
                        ));
                    }
                    emit(cli, &release_contract::parse_environment_binding(&text)?)
                }
                ContractKind::ResolvedPlacement => {
                    if payload.is_some() || now.is_some() {
                        return Err(usage(
                            "--payload and --now are valid only for saved-plan-envelope",
                        ));
                    }
                    emit(cli, &release_contract::parse_resolved_placement(&text)?)
                }
                ContractKind::SavedPlanEnvelope => {
                    let bytes =
                        payload
                            .as_deref()
                            .map(std::fs::read)
                            .transpose()
                            .map_err(|err| {
                                io(
                                    &payload.as_deref().map_or_else(
                                        || "saved plan".to_owned(),
                                        |path| path.display().to_string(),
                                    ),
                                    &err,
                                )
                            })?;
                    emit(
                        cli,
                        &release_contract::parse_saved_plan_envelope(
                            &text,
                            parse_now(now.as_deref())?,
                            bytes.as_deref(),
                        )?,
                    )
                }
            }
        }
    }
}

fn run_ledger(cli: &Cli, command: &LedgerCommand) -> Result<()> {
    match command {
        LedgerCommand::Append { entry, store } => {
            let entry: LedgerEntry = read_json(entry)?;
            JsonlLedger::new(store).append(&entry)?;
            emit(
                cli,
                &serde_json::json!({ "fence": entry.fence, "appended": true }),
            )
        }
        LedgerCommand::List { plane, store } => emit(cli, &JsonlLedger::new(store).list(plane)?),
        LedgerCommand::Verify {
            plane,
            store,
            binding_digest,
            actual,
        } => {
            let entries = JsonlLedger::new(store).list(plane)?;
            let actual: Readback = read_json(actual)?;
            ledger::verify_convergence(&entries, binding_digest, &actual)?;
            emit(cli, &serde_json::json!({ "converged": true }))
        }
    }
}

fn run_private_path(command: &PrivatePathCommand) -> Result<()> {
    match command {
        PrivatePathCommand::Check { target, policy } => {
            let policy: private_path::Policy = read_json(policy)?;
            let paths = aex_release_tool::graph::inputs::tracked_files(target)?;
            private_path::check(&policy, &paths, |path| {
                std::fs::read_to_string(target.join(path)).ok()
            })
        }
    }
}

fn run_migration(cli: &Cli, root: &Path, command: &MigrationCommand) -> Result<()> {
    match command {
        MigrationCommand::Bundle { out } => {
            let bundle = migration::build_bundle(root)?;
            if let Some(path) = out {
                write_canonical(path, &bundle)?;
            }
            emit(cli, &bundle)
        }
        MigrationCommand::Verify { bundle, head } => {
            let bundle: migration::Bundle = read_json(bundle)?;
            let head: migration::SchemaHead = read_json(head)?;
            migration::verify_bundle(&bundle, &head)?;
            emit(
                cli,
                &serde_json::json!({ "head": bundle.head, "valid": true }),
            )
        }
    }
}

fn run_policy(cli: &Cli, root: &Path, command: &PolicyCommand) -> Result<()> {
    match command {
        PolicyCommand::Terraform => {
            let scanned = policy::scan_terraform(root)?;
            emit(cli, &serde_json::json!({ "filesScanned": scanned }))
        }
        PolicyCommand::Workflows => emit(cli, &policy::lint_workflows(root)?),
        PolicyCommand::Dockerfile { file } => {
            let text = std::fs::read_to_string(file)
                .map_err(|err| io(&file.display().to_string(), &err))?;
            let violations = policy::scan_dockerfile(&file.display().to_string(), &text);
            if violations.is_empty() {
                emit(cli, &serde_json::json!({ "copyOnly": true }))
            } else {
                Err(ToolError::many(Exit::TerraformPolicy, violations))
            }
        }
    }
}

// --- shared helpers ----------------------------------------------------------

/// The janitor.
///
/// `sweep` exits `0` only when nothing the run created is past its deadline and
/// still there. Residue lands on `41`, the code the evidence hygiene rules
/// already share, so a lane that leaves production residue fails its own gate
/// rather than reporting a green sweep that removed nothing.
fn run_janitor(cli: &Cli, command: &JanitorCommand) -> Result<()> {
    match command {
        JanitorCommand::Scheme => {
            if !cli.quiet {
                print!("{}", janitor::describe_scheme());
            }
            Ok(())
        }
        JanitorCommand::Verify => janitor::verify_policy(),
        JanitorCommand::Sweep {
            plane,
            inventory,
            mode,
            now,
            out,
        } => {
            let text = std::fs::read_to_string(inventory)
                .map_err(|err| io(&inventory.display().to_string(), &err))?;
            let document = Inventory::parse(&text)?;
            if document.plane != *plane {
                return Err(usage(
                    "`--plane` names a plane the inventory does not: the document was taken \
                     from a different plane. Refusing rather than sweeping the wrong one.",
                ));
            }
            let instant = match now {
                None => time::OffsetDateTime::now_utc(),
                Some(text) => time::OffsetDateTime::parse(
                    text,
                    &time::format_description::well_known::Rfc3339,
                )
                .map_err(|err| usage(format!("`--now` is not RFC 3339: {err}")))?,
            };
            // A reclaiming sweep needs a credentialed adapter. None is compiled
            // in (OD-07), so `--mode reclaim` refuses per resource with a
            // stated reason rather than reporting a green no-op.
            let reclaimer: Box<dyn janitor::Reclaimer> = match mode {
                SweepMode::Report => Box::new(janitor::DryRun),
                SweepMode::Reclaim => Box::new(janitor::UnavailableAdapter),
            };
            let report = janitor::sweep(&document, reclaimer.as_ref(), *mode, instant);
            if let Some(path) = out {
                std::fs::write(path, canon::to_string(&report)?)
                    .map_err(|err| io(&path.display().to_string(), &err))?;
            }
            if !cli.quiet {
                eprintln!("{}", report.summary());
            }
            emit(cli, &report)?;
            let exit = report.exit();
            if exit == Exit::Ok {
                Ok(())
            } else {
                Err(ToolError::many(
                    exit,
                    report
                        .failed
                        .iter()
                        .map(|failure| {
                            Violation::new(
                                "janitor-residue",
                                format!(
                                    "{} `{}` from run {} was not reclaimed: {}",
                                    failure.kind, failure.identity, failure.run_id, failure.reason
                                ),
                            )
                        })
                        .chain(
                            report
                                .refused
                                .iter()
                                .filter(|entry| entry.is_residue)
                                .map(|entry| {
                                    Violation::new("janitor-residue", entry.detail.clone())
                                }),
                        )
                        .collect(),
                ))
            }
        }
    }
}

/// Which lockfile pins a unit kind's dependency versions.
fn lockfile_for(kind: &str) -> &'static str {
    if kind == "ts-lambda" || kind == "build-output" {
        "bun.lock"
    } else {
        "Cargo.lock"
    }
}

fn file_digest(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|err| io(&path.display().to_string(), &err))?;
    Ok(canon::digest_bytes(&bytes))
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn git_output(root: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|err| usage(format!("`git {}` could not be run: {err}", args.join(" "))))?;
    if !output.status.success() {
        return Err(usage(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn source_ref(root: &Path) -> Result<Option<String>> {
    if let Some(value) = std::env::var_os("GITHUB_REF") {
        let value = value.into_string().map_err(|_| {
            usage("`GITHUB_REF` is not valid Unicode and cannot identify the source ref")
        })?;
        return Ok(Some(value));
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .map_err(|err| {
            usage(format!(
                "`git symbolic-ref -q HEAD` could not be run: {err}"
            ))
        })?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ));
    }
    Ok(None)
}

/// The toolchain that produced the bytes, read from `rustc` rather than
/// declared. A declared toolchain is a claim; `rustc -vV` is a fact.
fn local_toolchain(target: &str) -> Result<artifact::Toolchain> {
    let output = std::process::Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|err| usage(format!("`rustc -vV` could not be run: {err}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let field = |name: &str| -> String {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{name}: ")))
            .unwrap_or_default()
            .to_owned()
    };
    let release = field("release");
    Ok(artifact::Toolchain {
        channel: release.clone(),
        rustc_version: release,
        rustc_commit_hash: field("commit-hash"),
        host: field("host"),
        target: target.to_owned(),
        components: Vec::new(),
        packager_version: None,
    })
}

/// Every repository file the graph attributes to a unit's input closure.
///
/// The closure is the forward-reachable set from the artifact node — its owning
/// package and every workspace dependency of it — plus every file the path map
/// classifies as repo-wide, because a change to the lockfile or the toolchain
/// pin shapes the bytes of everything.
fn input_closure(
    root: &Path,
    inputs: &GraphInputs,
    built: &verify::BuiltGraph,
    unit: &str,
) -> Result<BTreeMap<String, String>> {
    let start = built
        .graph
        .slot(&NodeId::artifact(unit))
        .ok_or_else(|| usage(format!("`{unit}` has no artifact node")))?;
    let mut reachable = built
        .graph
        .forward_closure(&[start], aex_release_tool::graph::EdgeKind::in_deploy_graph);
    reachable.push(start);
    let owned: std::collections::BTreeSet<&str> = reachable
        .iter()
        .map(|slot| built.graph.node(*slot).id.as_str())
        .collect();
    let npm_dirs = inputs.npm_dirs();
    let mut closure = BTreeMap::new();
    for file in &inputs.files {
        let include = match inputs.path_map.classify(file, &npm_dirs) {
            aex_release_tool::graph::pathmap::Classification::Owned { node, .. } => {
                owned.contains(node.as_str())
            }
            aex_release_tool::graph::pathmap::Classification::RepoWide { .. }
            | aex_release_tool::graph::pathmap::Classification::Router { .. } => true,
            _ => false,
        };
        if !include {
            continue;
        }
        let path = root.join(file);
        if !path.is_file() {
            continue;
        }
        closure.insert(file.clone(), file_digest(&path)?);
    }
    Ok(closure)
}

fn emit<T: serde::Serialize>(cli: &Cli, value: &T) -> Result<()> {
    if cli.quiet {
        return Ok(());
    }
    println!("{}", canon::to_string(value)?);
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text =
        std::fs::read_to_string(path).map_err(|err| io(&path.display().to_string(), &err))?;
    serde_json::from_str(&text).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "document-unparseable",
            format!("`{}`: {err}", path.display()),
        )
    })
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text =
        std::fs::read_to_string(path).map_err(|err| io(&path.display().to_string(), &err))?;
    toml::from_str(&text).map_err(|err| {
        ToolError::single(
            Exit::Usage,
            "document-unparseable",
            format!("`{}`: {err}", path.display()),
        )
    })
}

fn read_units(root: &Path) -> Result<aex_release_tool::graph::inputs::Units> {
    read_toml(&root.join("release/units.toml"))
}

fn write_canonical<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|err| io(&parent.display().to_string(), &err))?;
    }
    std::fs::write(path, canon::to_file_bytes(value)?)
        .map_err(|err| io(&path.display().to_string(), &err))
}

fn append_text(path: &Path, text: &str) -> Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| io(&path.display().to_string(), &err))?;
    file.write_all(text.as_bytes())
        .map_err(|err| io(&path.display().to_string(), &err))
}

fn parse_now(value: Option<&str>) -> Result<time::OffsetDateTime> {
    match value {
        None => Ok(time::OffsetDateTime::now_utc()),
        Some(text) => {
            time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                .map_err(|err| usage(format!("`--now {text}` is not RFC 3339: {err}")))
        }
    }
}

/// Resolve the changed path set from a commit range or an explicit list.
///
/// A range and an explicit list are different questions and the caller must
/// pick one. Silently defaulting to "no paths" would let an affected selection
/// run nothing and report success.
fn resolve_changed(
    root: &Path,
    base: Option<&str>,
    head: Option<&str>,
    paths: Option<&[String]>,
) -> Result<Vec<String>> {
    if let Some(paths) = paths {
        return Ok(paths.to_vec());
    }
    let (Some(base), Some(head)) = (base, head) else {
        return Err(ToolError::single(
            Exit::RoutingUndecidable,
            "routing-no-change-information",
            "pass either --base and --head, or --paths; routing cannot be computed from \
             nothing",
        ));
    };
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--name-only", "-z", &format!("{base}..{head}")])
        .output()
        .map_err(|err| {
            ToolError::single(
                Exit::RoutingUndecidable,
                "routing-git-unavailable",
                format!("`git diff` could not be run: {err}"),
            )
        })?;
    if !output.status.success() {
        return Err(ToolError::single(
            Exit::RoutingUndecidable,
            "routing-git-unavailable",
            format!(
                "`git diff {base}..{head}` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_documented_subcommand_group_is_reachable() {
        let command = Cli::command();
        let mut names: Vec<&str> = command
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "admit",
                "artifact",
                "contract",
                "evidence",
                "graph",
                "janitor",
                "ledger",
                "manifest",
                "migration",
                "plan",
                "policy",
                "private-path",
                "schema",
                "selftest",
                "verification",
            ]
        );
    }

    #[test]
    fn the_six_wrappers_the_handoff_left_unwired_are_reachable() {
        // `references/rewrite/delivery.md` §4 named these as library functions
        // with no command in front of them. A group that exists while its
        // leaves do not is exactly the shape nobody notices is missing.
        let command = Cli::command();
        let leaves = |group: &str| -> Vec<String> {
            command
                .get_subcommands()
                .find(|candidate| candidate.get_name() == group)
                .unwrap_or_else(|| panic!("`{group}` is not a subcommand group"))
                .get_subcommands()
                .map(|leaf| leaf.get_name().to_owned())
                .collect()
        };
        for (group, leaf) in [
            ("artifact", "describe"),
            ("evidence", "new"),
            ("evidence", "attach"),
            ("verification", "new"),
            ("manifest", "new"),
            ("manifest", "diff"),
        ] {
            assert!(
                leaves(group).iter().any(|name| name == leaf),
                "`{group} {leaf}` is not reachable"
            );
        }
    }
}
