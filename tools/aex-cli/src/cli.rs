#![allow(
    missing_docs,
    reason = "clap derives the customer-facing documentation from these declarations"
)]

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::output::OutputFormat;

/// The native client for the AEX v1 public API.
#[derive(Debug, Parser)]
#[command(name = "aex", version, about, disable_help_subcommand = true)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent clap switches are boolean by contract"
)]
pub struct Cli {
    /// Print the declarative command registry as JSON.
    #[arg(long, hide = true)]
    pub dump_command_registry: bool,
    /// Workspace API key or account token.
    #[arg(long, global = true, env = "AEX_API_KEY")]
    pub api_key: Option<String>,
    /// Selected local profile.
    #[arg(long, global = true, env = "AEX_PROFILE", default_value = "default")]
    pub profile: String,
    /// Explicit configuration file.
    #[arg(long, global = true, env = "AEX_CONFIG")]
    pub config: Option<PathBuf>,
    /// Bootstrap-plane URL.
    #[arg(long, global = true, env = "AEX_CENTRAL_URL")]
    pub central_url: Option<String>,
    /// Workspace-plane URL.
    #[arg(long, global = true, env = "AEX_REGIONAL_URL")]
    pub regional_url: Option<String>,
    /// Workspace binding for an account token.
    #[arg(long, global = true, env = "AEX_WORKSPACE_ID")]
    pub workspace: Option<String>,
    /// Output representation.
    #[arg(long, global = true, env = "AEX_OUTPUT", value_enum)]
    pub output: Option<OutputFormat>,
    /// Suppress non-error diagnostics.
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// Increase diagnostic detail.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Disable safe retry for this invocation.
    #[arg(long, global = true)]
    pub no_retry: bool,
    /// Client deadline for wait-style commands.
    #[arg(long, global = true)]
    pub wait_timeout: Option<u64>,
    /// Durable operation polling interval.
    #[arg(long, global = true, default_value_t = 1_000)]
    pub poll_interval: u64,
    /// Caller-supplied replay identity.
    #[arg(long, global = true)]
    pub idempotency_key: Option<String>,
    /// Caller-supplied durable operation identity.
    #[arg(long, global = true)]
    pub operation_id: Option<String>,
    /// Return immediately after durable admission.
    #[arg(long, global = true)]
    pub detach: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    Org {
        #[command(subcommand)]
        command: OrgCommand,
    },
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Message {
        #[command(subcommand)]
        command: MessageCommand,
    },
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
    Approval {
        #[command(subcommand)]
        command: ApprovalCommand,
    },
    Operation {
        #[command(subcommand)]
        command: OperationCommand,
    },
    File {
        #[command(subcommand)]
        command: FileCommand,
    },
    Registry {
        #[command(subcommand)]
        command: RegistryCommand,
    },
    Upload {
        #[command(subcommand)]
        command: UploadCommand,
    },
    Secret {
        #[command(subcommand)]
        command: SecretCommand,
    },
    Limit {
        #[command(subcommand)]
        command: LimitCommand,
    },
    Observe {
        #[command(subcommand)]
        command: ObserveCommand,
    },
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommand,
    },
    Usage {
        #[command(subcommand)]
        command: UsageCommand,
    },
    Billing {
        #[command(subcommand)]
        command: BillingCommand,
    },
    /// Generate shell completions.
    Completions {
        #[arg(value_enum)]
        shell: CompletionShell,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Version,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    Powershell,
    Elvish,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    Login,
    Logout,
    Status,
}
#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    Get {
        #[arg(long)]
        organization: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum OrgCommand {
    List(PageArgs),
    Create {
        #[arg(long)]
        name: String,
    },
    Get {
        organization: String,
    },
    Member {
        #[command(subcommand)]
        command: MemberCommand,
    },
    Invite {
        #[command(subcommand)]
        command: InviteCommand,
    },
}
#[derive(Debug, Subcommand)]
pub enum MemberCommand {
    List { organization: String },
}
#[derive(Debug, Subcommand)]
pub enum InviteCommand {
    Create {
        organization: String,
        #[arg(long)]
        email: String,
        #[arg(long)]
        role: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    List(PageArgs),
    Create {
        #[arg(long)]
        organization: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        region: String,
    },
    Get {
        workspace: String,
    },
    Delete {
        workspace: String,
        #[arg(long)]
        confirm: String,
    },
    Current,
}
#[derive(Debug, Subcommand)]
pub enum KeyCommand {
    List {
        #[arg(long)]
        workspace: String,
    },
    Create {
        #[arg(long)]
        workspace: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        scope: Vec<String>,
    },
    Revoke {
        key: String,
        #[arg(long)]
        if_revision: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    Create(SessionCreateArgs),
    List(PageArgs),
    Get {
        session: String,
    },
    Stop {
        session: String,
    },
    Persist {
        session: String,
    },
    Fork {
        session: String,
    },
    Delete {
        session: String,
    },
    Discard {
        session: String,
    },
    Rebind {
        session: String,
        #[arg(long)]
        secret: Vec<String>,
    },
}
#[derive(Debug, Args)]
pub struct SessionCreateArgs {
    #[arg(long)]
    pub provider: String,
    #[arg(long)]
    pub model: String,
    #[arg(long)]
    pub credential: Option<String>,
    #[arg(long)]
    pub compute: Option<String>,
    #[arg(long)]
    pub network: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum MessageCommand {
    List {
        session: String,
    },
    Send {
        session: String,
        text: Option<String>,
    },
}
#[derive(Debug, Subcommand)]
pub enum RunCommand {
    List { session: String },
    Get { session: String, run: String },
    Wait { session: String, run: String },
}
#[derive(Debug, Subcommand)]
pub enum ApprovalCommand {
    List {
        session: String,
    },
    Get {
        session: String,
        approval: String,
    },
    Respond {
        session: String,
        approval: String,
        #[arg(long)]
        decision: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum OperationCommand {
    List,
    Get { operation: String },
    Wait { operation: String },
    Cancel { operation: String },
}

#[derive(Debug, Subcommand)]
pub enum FileCommand {
    Persisted {
        #[command(subcommand)]
        command: FileModeCommand,
    },
    Live {
        #[command(subcommand)]
        command: FileModeCommand,
    },
}
#[derive(Debug, Subcommand)]
pub enum FileModeCommand {
    List {
        session: String,
    },
    Stat {
        session: String,
        path: String,
    },
    Download {
        session: String,
        path: String,
        #[arg(long)]
        destination: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RegistryCommand {
    File {
        #[command(subcommand)]
        command: RegistryFileCommand,
    },
    Skill {
        #[command(subcommand)]
        command: RegistryCrudCommand,
    },
    Tool {
        #[command(subcommand)]
        command: RegistryCrudCommand,
    },
    Instruction {
        #[command(subcommand)]
        command: RegistryCrudCommand,
    },
    McpServer {
        #[command(subcommand)]
        command: RegistryCrudCommand,
    },
}
#[derive(Debug, Subcommand)]
pub enum RegistryCrudCommand {
    List,
    Get {
        name: String,
    },
    Set {
        name: String,
        #[arg(long)]
        request: String,
    },
    Delete {
        name: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum RegistryFileCommand {
    List,
    Get {
        name: String,
    },
    Set {
        name: String,
        #[arg(long)]
        request: String,
    },
    Delete {
        name: String,
    },
    Download {
        name: String,
        #[arg(long)]
        destination: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum UploadCommand {
    Create {
        #[arg(long)]
        size_bytes: u64,
        #[arg(long)]
        sha256: String,
        #[arg(long)]
        content_type: String,
    },
    Parts {
        upload: String,
        #[arg(long)]
        part: Vec<u32>,
    },
    Complete {
        upload: String,
        #[arg(long)]
        request: String,
    },
    Abort {
        upload: String,
    },
    Put {
        path: PathBuf,
        #[arg(long)]
        content_type: Option<String>,
    },
}
#[derive(Debug, Subcommand)]
pub enum SecretCommand {
    List,
    Get {
        name: String,
    },
    Set {
        name: String,
        #[arg(long)]
        value: Option<String>,
        #[arg(long)]
        value_stdin: bool,
    },
    Delete {
        name: String,
    },
    Revoke {
        name: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum LimitCommand {
    List,
    Get { limit: String },
}

#[derive(Debug, Subcommand)]
pub enum ObserveCommand {
    Events(SignalArgs),
    Logs(SignalArgs),
    Spans(SignalArgs),
    Metrics(SignalArgs),
    Traces(SignalArgs),
    Telemetry(SignalArgs),
}
#[derive(Debug, Args)]
pub struct SignalArgs {
    #[command(subcommand)]
    pub command: SignalCommand,
}
#[derive(Debug, Subcommand)]
pub enum SignalCommand {
    Query {
        #[arg(long)]
        query: String,
    },
    Stream {
        #[arg(long)]
        query: String,
    },
    Listen {
        #[arg(long)]
        query: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum TelemetryCommand {
    Gap {
        #[command(subcommand)]
        command: GapCommand,
    },
    Export {
        #[command(subcommand)]
        command: ExportCommand,
    },
    Otlp {
        #[command(subcommand)]
        command: OtlpCommand,
    },
}
#[derive(Debug, Subcommand)]
pub enum GapCommand {
    Query {
        #[arg(long)]
        query: String,
    },
    Get {
        gap: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum ExportCommand {
    Create {
        #[arg(long)]
        query: String,
    },
    Get {
        export: String,
    },
    Download {
        export: String,
        #[arg(long)]
        destination: String,
    },
    Revoke {
        export: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum OtlpCommand {
    Logs {
        #[arg(long)]
        request: String,
    },
    Traces {
        #[arg(long)]
        request: String,
    },
    Metrics {
        #[arg(long)]
        request: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum UsageCommand {
    Query {
        #[arg(long)]
        query: String,
        #[arg(long)]
        organization: Option<String>,
    },
}
#[derive(Debug, Subcommand)]
pub enum BillingCommand {
    Balance,
    Topup {
        organization: String,
        #[arg(long)]
        amount_microusd: u64,
    },
    Portal {
        organization: String,
    },
    Autotopup {
        #[command(subcommand)]
        command: AutoTopupCommand,
    },
    Statement {
        #[command(subcommand)]
        command: StatementCommand,
    },
}
#[derive(Debug, Subcommand)]
pub enum AutoTopupCommand {
    Get {
        organization: String,
    },
    Set {
        organization: String,
        #[arg(long)]
        if_revision: u64,
    },
}
#[derive(Debug, Subcommand)]
pub enum StatementCommand {
    List {
        organization: String,
    },
    Get {
        organization: String,
        statement: String,
    },
    Download {
        organization: String,
        statement: String,
        #[arg(long)]
        destination: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    Path,
    List,
    Get { key: String },
    Set { key: String, value: String },
    Unset { key: String },
}

#[derive(Debug, Args)]
pub struct PageArgs {
    #[arg(long)]
    pub cursor: Option<String>,
    #[arg(long)]
    pub limit: Option<u16>,
}
