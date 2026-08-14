#![allow(
    missing_docs,
    reason = "clap derives the customer-facing documentation from these declarations"
)]

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};

use crate::output::OutputFormat;

/// The native client for the session-centered AEX public API.
#[derive(Debug, Parser)]
#[command(name = "aex", version, about, disable_help_subcommand = true)]
pub struct Cli {
    /// Print the declarative command registry as JSON.
    #[arg(long, hide = true)]
    pub dump_command_registry: bool,
    /// Selected non-secret local profile.
    #[arg(long, global = true, env = "AEX_PROFILE", default_value = "default")]
    pub profile: String,
    /// Explicit configuration file.
    #[arg(long, global = true, env = "AEX_CONFIG")]
    pub config: Option<PathBuf>,
    /// Central API origin.
    #[arg(long, global = true, env = "AEX_CENTRAL_URL")]
    pub central_url: Option<String>,
    /// Regional session API origin.
    #[arg(long, global = true, env = "AEX_REGIONAL_URL")]
    pub regional_url: Option<String>,
    /// Output representation.
    #[arg(long, global = true, env = "AEX_OUTPUT", value_enum)]
    pub output: Option<OutputFormat>,
    /// Suppress non-error diagnostics.
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// Increase diagnostic detail without printing credentials or signed URLs.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Caller-supplied replay identity for write commands.
    #[arg(long, global = true)]
    pub idempotency_key: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// The complete launch command surface.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Provision or inspect the signed-in personal account and workspace.
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Mint, list, and revoke fixed-workspace API keys.
    ApiKey {
        #[command(subcommand)]
        command: ApiKeyCommand,
    },
    /// Create and manage durable sessions.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Send, list, and follow session messages.
    Message {
        #[command(subcommand)]
        command: MessageCommand,
    },
    /// Manage current latest-only workspace files.
    File {
        #[command(subcommand)]
        command: FileCommand,
    },
    /// Follow, replay, and download retained session telemetry.
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommand,
    },
    /// Inspect balance/cards/usage and open Stripe-hosted flows.
    Billing {
        #[command(subcommand)]
        command: BillingCommand,
    },
    /// Inspect and update non-secret local configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Print CLI and contract identity.
    Version,
}

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    /// Sign in with Google, provision the personal account, and mint its first workspace key.
    Create {
        /// Display name for the first workspace key.
        #[arg(long)]
        name: String,
    },
    /// Read the signed-in personal account and fixed workspace.
    Bootstrap,
}

#[derive(Debug, Subcommand)]
pub enum ApiKeyCommand {
    /// Mint a workspace API key. Its secret value is returned only by this command.
    Create {
        /// Display name for the key.
        #[arg(long)]
        name: String,
        /// Workspace scope to grant. Repeat to grant several; defaults to all workspace scopes.
        #[arg(long, value_parser = ["sessions:read", "sessions:write", "sessions:delete", "resources:read", "resources:write"])]
        scope: Vec<String>,
    },
    /// List API-key metadata for the fixed workspace.
    List(#[command(flatten)] PageArgs),
    /// Revoke an API key by ID.
    Revoke { api_key: String },
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    Create(SessionCreateArgs),
    List(SessionListArgs),
    Get {
        session: String,
    },
    Terminate {
        session: String,
    },
    Delete {
        session: String,
        /// Required acknowledgement of irreversible session-content deletion.
        #[arg(long, value_parser = ["delete"])]
        confirm: String,
    },
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("provider-secret")
        .required(true)
        .args(["provider_key_env", "provider_key_stdin"])
))]
pub struct SessionCreateArgs {
    /// Official provider family.
    #[arg(long, value_parser = ["openai", "anthropic", "deepseek", "xai", "meta", "moonshotai", "alibaba"])]
    pub provider: String,
    /// Provider-native model identifier.
    #[arg(long)]
    pub model: String,
    /// Name of the environment variable containing the write-only provider key.
    #[arg(long, conflicts_with = "provider_key_stdin")]
    pub provider_key_env: Option<String>,
    /// Read the write-only provider key from stdin.
    #[arg(long, conflicts_with = "provider_key_env")]
    pub provider_key_stdin: bool,
    /// Name of an environment variable containing the MCP server JSON array.
    #[arg(long, conflicts_with = "mcp_json_stdin")]
    pub mcp_json_env: Option<String>,
    /// Read the MCP server JSON array, including any secrets, from stdin.
    #[arg(long, conflicts_with = "mcp_json_env")]
    pub mcp_json_stdin: bool,
    /// Resolve and mount a current file as NAME=/workspace/path.
    #[arg(long = "mount")]
    pub mounts: Vec<String>,
    /// Explicitly create a no-sandbox session.
    #[arg(long)]
    pub no_sandbox: bool,
    /// Prepaid reservation ceiling in whole cents.
    #[arg(long)]
    pub max_spend_cents: Option<u64>,
}

#[derive(Debug, Args)]
pub struct SessionListArgs {
    #[command(flatten)]
    pub page: PageArgs,
    #[arg(long, value_parser = ["idle", "running", "terminating", "terminated", "deleting"])]
    pub status: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum MessageCommand {
    Send {
        session: String,
        /// Message text. When omitted, UTF-8 text is read from stdin.
        text: Option<String>,
        /// Per-message spend fence in whole cents.
        #[arg(long)]
        max_spend_cents: Option<u64>,
        /// JSON Schema for a native structured final response.
        #[arg(long)]
        response_schema: Option<PathBuf>,
    },
    List {
        session: String,
        #[command(flatten)]
        page: PageArgs,
    },
    Tail {
        session: String,
        /// Resume after this message stream sequence.
        #[arg(long)]
        after: Option<u128>,
    },
}

#[derive(Debug, Subcommand)]
pub enum FileCommand {
    /// Replace a current file with inline local/stdin bytes or an HTTPS URL.
    Put {
        name: String,
        #[arg(long, conflicts_with = "url")]
        path: Option<PathBuf>,
        #[arg(long, conflicts_with = "path")]
        url: Option<String>,
        #[arg(long, default_value = "application/octet-stream")]
        media_type: String,
        #[arg(long)]
        executable: bool,
    },
    /// Multipart-upload arbitrary local bytes as the current logical file.
    Upload {
        name: String,
        path: PathBuf,
        #[arg(long, default_value = "application/octet-stream")]
        media_type: String,
    },
    List(PageArgs),
    Get {
        name: String,
    },
    Download {
        name: String,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        force: bool,
    },
    Delete {
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum TelemetryCommand {
    Tail {
        session: String,
        #[arg(long)]
        after: Option<u128>,
    },
    Replay {
        session: String,
        #[arg(long)]
        after: Option<u128>,
        #[arg(long)]
        limit: Option<u32>,
    },
    Download {
        session: String,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        from_sequence: Option<u128>,
        #[arg(long)]
        to_sequence: Option<u128>,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum BillingCommand {
    Balance,
    Cards,
    SetupCard {
        /// Explicit consent to save the card for future prepaid payments.
        #[arg(long)]
        consent: bool,
        #[arg(long)]
        success_url: Option<String>,
        #[arg(long)]
        cancel_url: Option<String>,
    },
    RemoveCard {
        payment_method: String,
    },
    Topup {
        #[arg(long)]
        amount_cents: u64,
        #[arg(long)]
        success_url: Option<String>,
        #[arg(long)]
        cancel_url: Option<String>,
    },
    Transactions(PageArgs),
    Usage(BillingUsageArgs),
}

#[derive(Debug, Args)]
pub struct BillingUsageArgs {
    #[arg(long, value_parser = ["model", "runtime", "storage", "transfer"])]
    pub category: Option<String>,
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long)]
    pub from: Option<String>,
    #[arg(long)]
    pub to: Option<String>,
    #[command(flatten)]
    pub page: PageArgs,
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
    pub limit: Option<u32>,
}
