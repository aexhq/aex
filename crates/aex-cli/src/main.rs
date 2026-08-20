//! `aex` — the command line for the Aex platform.
//!
//! The slice-4 gate flow, end to end: `aex signup` → `aex topup` (pay at the printed checkout
//! URL) → `aex keys create` → `aex session new` + `aex session send` → `aex usage`.
//!
//! Credentials: the account token and API key live in the config file (or `AEX_ACCOUNT_TOKEN`
//! / `AEX_API_KEY` env, which win). Model keys are BYOK and are read from an env var named by
//! `--key-env` — never from an argument, so they stay out of shell history and process lists.

use std::io::Write as _;
use std::path::PathBuf;

use aex_contracts::session::Event;
use aex_sdk::Client;
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "aex",
    version,
    about = "Aex: durable agent sessions with prepaid usage"
)]
struct Cli {
    /// Control-plane URL (default: AEX_BASE_URL, then the config file, then http://127.0.0.1:8600).
    #[arg(long, global = true)]
    base_url: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sign up; prints and stores the account token (shown exactly once).
    Signup {
        #[arg(long)]
        email: String,
        /// Print the token but do not write the config file.
        #[arg(long)]
        no_save: bool,
    },
    /// Show the authenticated account.
    Account,
    /// Show the prepaid balance (meters usage first).
    Balance,
    /// Show the public rate card.
    Rates,
    /// The bill: every session's rated line, storage meters included.
    Usage,
    /// Top up the prepaid balance; pay at the printed checkout URL.
    Topup {
        /// Dollars, e.g. 10 or 12.50 (minimum 10.00).
        #[arg(long)]
        usd: String,
        /// Do not wait for the payment to settle.
        #[arg(long)]
        no_wait: bool,
    },
    /// List top-ups.
    Topups,
    /// API keys.
    Keys {
        #[command(subcommand)]
        cmd: KeysCmd,
    },
    /// Sessions.
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },
}

#[derive(Subcommand)]
enum KeysCmd {
    /// Create an API key; prints and stores the secret (shown exactly once).
    Create {
        #[arg(long)]
        name: String,
        /// Print the secret but do not write the config file.
        #[arg(long)]
        no_save: bool,
    },
    /// List keys (never the secrets).
    List,
    /// Revoke a key immediately.
    Revoke { key_id: String },
}

#[derive(Subcommand)]
enum SessionCmd {
    /// Create a session (BYOK: the model key comes from --key-env).
    New {
        /// Model provider: openai | anthropic | deepseek | moonshot | xai | openai_compatible.
        #[arg(long)]
        provider: String,
        /// Provider model id, e.g. claude-sonnet-5.
        #[arg(long)]
        model: String,
        /// Env var holding the provider API key (default by provider, e.g. ANTHROPIC_API_KEY).
        #[arg(long)]
        key_env: Option<String>,
        /// Provider endpoint override (required for openai_compatible).
        #[arg(long)]
        model_base_url: Option<String>,
        /// Hand shape: 1gb | 2gb | 4gb | 8gb.
        #[arg(long)]
        shape: Option<String>,
        /// System prompt.
        #[arg(long)]
        system: Option<String>,
    },
    /// List sessions.
    List,
    /// Show one session.
    Get { session_id: String },
    /// Send a message and stream the turn until it completes.
    Send { session_id: String, message: String },
    /// End now: release the hand, keep the workspace.
    End { session_id: String },
    /// Workspace files.
    Files {
        #[command(subcommand)]
        cmd: FilesCmd,
    },
    /// Persist a workspace file as a named durable artifact.
    Persist {
        session_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        media_type: Option<String>,
    },
    /// Durable artifacts.
    Artifacts {
        #[command(subcommand)]
        cmd: ArtifactsCmd,
    },
    /// Delete (irreversible): hand, workspace, artifacts, journal.
    Delete { session_id: String },
}

#[derive(Subcommand)]
enum FilesCmd {
    /// List a workspace path.
    List {
        session_id: String,
        #[arg(long, default_value = "/workspace")]
        path: String,
        #[arg(long)]
        recursive: bool,
    },
    /// Download exact bytes to a local file.
    Get {
        session_id: String,
        path: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Upload a local file as an absolute workspace overwrite.
    Put {
        session_id: String,
        path: String,
        #[arg(long)]
        input: PathBuf,
    },
}

#[derive(Subcommand)]
enum ArtifactsCmd {
    List { session_id: String },
    Get { session_id: String, name: String },
}

// ---- config file ----

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct FileConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
}

fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("AEX_CONFIG") {
        return p.into();
    }
    let home = std::env::var("APPDATA")
        .or_else(|_| std::env::var("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config")
        });
    home.join("aex").join("config.json")
}

fn load_config() -> FileConfig {
    std::fs::read(config_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &FileConfig) -> anyhow::Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(cfg)? + "\n")?;
    println!("(saved to {})", path.display());
    Ok(())
}

fn client(cli: &Cli, cfg: &FileConfig) -> Client {
    let base = cli
        .base_url
        .clone()
        .or_else(|| std::env::var("AEX_BASE_URL").ok())
        .or_else(|| cfg.base_url.clone())
        .unwrap_or_else(|| "http://127.0.0.1:8600".into());
    let mut c = Client::new(base);
    if let Some(at) = std::env::var("AEX_ACCOUNT_TOKEN")
        .ok()
        .or_else(|| cfg.account_token.clone())
    {
        c = c.with_account_token(at);
    }
    if let Some(sk) = std::env::var("AEX_API_KEY")
        .ok()
        .or_else(|| cfg.api_key.clone())
    {
        c = c.with_api_key(sk);
    }
    c
}

// ---- money display ----

fn musd(v: i64) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let a = v.unsigned_abs();
    format!("{sign}${}.{:06}", a / 1_000_000, a % 1_000_000)
}

fn parse_usd_to_cents(s: &str) -> anyhow::Result<i64> {
    let (dollars, cents) = match s.split_once('.') {
        None => (s, "0"),
        Some((d, c)) if c.len() <= 2 && !c.is_empty() => (d, c),
        Some(_) => anyhow::bail!("--usd takes at most two decimals, e.g. 12.50"),
    };
    let d: i64 = dollars
        .parse()
        .map_err(|_| anyhow::anyhow!("--usd: not a number: {s}"))?;
    let mut c: i64 = cents
        .parse()
        .map_err(|_| anyhow::anyhow!("--usd: not a number: {s}"))?;
    if cents.len() == 1 {
        c *= 10;
    }
    Ok(d * 100 + c)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut cfg = load_config();
    let c = client(&cli, &cfg);
    match cli.cmd {
        Cmd::Signup { email, no_save } => {
            let created = c.signup(&email).await?;
            println!(
                "account: {}  ({})",
                *created.account.id, created.account.email
            );
            println!("account token (SHOWN ONCE — it manages your money and keys):");
            println!("  {}", *created.account_token);
            if !no_save {
                cfg.account_token = Some(created.account_token.to_string());
                save_config(&cfg)?;
            }
        }
        Cmd::Account => {
            let a = c.account().await?;
            println!("{}  {}  created {}", *a.id, a.email, *a.created_at);
            println!(
                "limits: {} concurrent sessions, {} creates/hour",
                a.limits.max_concurrent_sessions, a.limits.session_creates_per_hour
            );
        }
        Cmd::Balance => {
            let b = c.balance().await?;
            println!(
                "balance: ${}  ({} micro-USD, metered to {})",
                *b.usd, b.microusd.0, *b.metered_to
            );
        }
        Cmd::Rates => {
            let r = c.rates().await?;
            println!("compute, per second while running (baseline; bursts free):");
            println!(
                "  {}/vCPU-hour + {}/GB-hour",
                musd(r.vcpu_hour_microusd.0),
                musd(r.gb_hour_microusd.0)
            );
            println!(
                "  1gb {}/h · 2gb {}/h · 4gb {}/h · 8gb {}/h",
                musd(r.vcpu_hour_microusd.0 / 2 + r.gb_hour_microusd.0),
                musd(r.vcpu_hour_microusd.0 + r.gb_hour_microusd.0 * 2),
                musd(r.vcpu_hour_microusd.0 * 2 + r.gb_hour_microusd.0 * 4),
                musd(r.vcpu_hour_microusd.0 * 4 + r.gb_hour_microusd.0 * 8)
            );
            println!("storage, per GB-month ({}h):", r.month_hours);
            println!(
                "  suspended {}  ·  workspace+artifacts {}",
                musd(r.suspended_gb_month_microusd.0),
                musd(r.workspace_gb_month_microusd.0)
            );
            println!(
                "web_search: {} per query",
                musd(r.web_search_query_microusd.0)
            );
        }
        Cmd::Usage => {
            let u = c.usage().await?;
            println!(
                "{:<32} {:<5} {:<8} {:>10} {:>8} {:>12} {:>12} {:>12} {:>12}",
                "SESSION",
                "SHAPE",
                "STATE",
                "RUNNING",
                "SEARCH",
                "COMPUTE",
                "STORAGE",
                "WEB",
                "TOTAL"
            );
            for s in &u.sessions {
                println!(
                    "{:<32} {:<5} {:<8} {:>9.1}s {:>8} {:>12} {:>12} {:>12} {:>12}",
                    *s.session_id,
                    s.shape,
                    s.state,
                    s.running_ms as f64 / 1000.0,
                    s.web_search_queries,
                    musd(s.compute_microusd.0),
                    musd(s.storage_microusd.0),
                    musd(s.web_search_microusd.0),
                    musd(s.total_microusd.0),
                );
            }
            println!(
                "rated total {}   balance {}   (metered to {})",
                musd(u.total_microusd.0),
                musd(u.balance_microusd.0),
                *u.metered_to
            );
        }
        Cmd::Topup { usd, no_wait } => {
            let cents = parse_usd_to_cents(&usd)?;
            let t = c.create_topup(cents).await?;
            println!(
                "top-up {}: ${}.{:02}  [{}]",
                *t.id,
                cents / 100,
                cents % 100,
                t.status
            );
            if let Some(url) = &t.checkout_url {
                println!("pay here:\n  {url}");
            }
            if !no_wait {
                print!("waiting for the payment to settle");
                std::io::stdout().flush().ok();
                for _ in 0..300 {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let t = c.get_topup(&t.id).await?;
                    if t.status.to_string() != "pending" {
                        println!("\ntop-up {}: {}", *t.id, t.status);
                        let b = c.balance().await?;
                        println!("balance: ${}", *b.usd);
                        return Ok(());
                    }
                    print!(".");
                    std::io::stdout().flush().ok();
                }
                println!("\nstill pending; check later with `aex topups`");
            }
        }
        Cmd::Topups => {
            for t in c.list_topups().await?.data {
                println!(
                    "{}  ${}.{:02}  {}  created {}{}",
                    *t.id,
                    t.amount_cents.get() / 100,
                    t.amount_cents.get() % 100,
                    t.status,
                    *t.created_at,
                    t.paid_at
                        .as_deref()
                        .map(|p| format!("  paid {p}"))
                        .unwrap_or_default()
                );
            }
        }
        Cmd::Keys { cmd } => match cmd {
            KeysCmd::Create { name, no_save } => {
                let k = c.create_key(&name).await?;
                println!("key {} ({})", *k.key.id, *k.key.name);
                println!("secret (SHOWN ONCE — it runs sessions on your balance):");
                println!("  {}", *k.secret);
                if !no_save {
                    cfg.api_key = Some(k.secret.to_string());
                    save_config(&cfg)?;
                }
            }
            KeysCmd::List => {
                for k in c.list_keys().await?.data {
                    println!(
                        "{}  {:<20} {}...  created {}{}{}",
                        *k.id,
                        *k.name,
                        *k.prefix,
                        *k.created_at,
                        k.last_used_at
                            .as_deref()
                            .map(|t| format!("  last used {t}"))
                            .unwrap_or_default(),
                        k.revoked_at
                            .as_deref()
                            .map(|t| format!("  REVOKED {t}"))
                            .unwrap_or_default()
                    );
                }
            }
            KeysCmd::Revoke { key_id } => {
                c.revoke_key(&key_id).await?;
                println!("revoked {key_id}");
            }
        },
        Cmd::Session { cmd } => match cmd {
            SessionCmd::New {
                provider,
                model,
                key_env,
                model_base_url,
                shape,
                system,
            } => {
                let key_env = key_env.unwrap_or_else(|| match provider.as_str() {
                    "anthropic" => "ANTHROPIC_API_KEY".into(),
                    "openai" => "OPENAI_API_KEY".into(),
                    "deepseek" => "DEEPSEEK_API_KEY".into(),
                    p => format!("{}_API_KEY", p.to_uppercase()),
                });
                let api_key = std::env::var(&key_env).map_err(|_| {
                    anyhow::anyhow!("model key env {key_env} is not set (BYOK; see --key-env)")
                })?;
                let mut model_cfg =
                    json!({"provider": provider, "name": model, "api_key": api_key});
                if let Some(u) = model_base_url {
                    model_cfg["base_url"] = json!(u);
                }
                let mut body = json!({"model": model_cfg});
                if let Some(s) = shape {
                    body["hand"] = json!({"shape": s});
                }
                if let Some(s) = system {
                    body["system_prompt"] = json!(s);
                }
                let s = c.create_session(&body).await?;
                println!(
                    "session {}  (hand {}, shape {})",
                    *s.id, s.hand.state, s.hand.shape
                );
            }
            SessionCmd::List => {
                for s in c.list_sessions().await?.data {
                    println!(
                        "{}  {:<6} hand {:<9} turns {:<3} created {}",
                        *s.id, s.state, s.hand.state, s.turns, *s.created_at
                    );
                }
            }
            SessionCmd::Get { session_id } => {
                let s = c.get_session(&session_id).await?;
                println!("{}", serde_json::to_string_pretty(&s)?);
            }
            SessionCmd::Send {
                session_id,
                message,
            } => {
                let accepted = c
                    .send_message(&session_id, &json!({"content": message}))
                    .await?;
                let after = i64::try_from(accepted.seq.get()).unwrap_or(i64::MAX) - 1;
                let turn = accepted.turn_id.clone();
                let mut saw_delta = false;
                c.events(&session_id, after, true, |ev| {
                    render_event(&ev, &turn, &mut saw_delta)
                })
                .await?;
            }
            SessionCmd::End { session_id } => {
                let s = c.end_session(&session_id).await?;
                println!("ended: hand {}, workspace kept", s.hand.state);
            }
            SessionCmd::Files { cmd } => match cmd {
                FilesCmd::List {
                    session_id,
                    path,
                    recursive,
                } => {
                    let files = c.list_files(&session_id, &path, recursive).await?;
                    println!(
                        "source {}{}",
                        files.source,
                        files
                            .synced_at
                            .map(|at| format!("  synced {at}"))
                            .unwrap_or_default()
                    );
                    for file in files.data {
                        println!(
                            "{:<7} {:>12}  {}",
                            file.kind,
                            file.size
                                .map(|size| size.to_string())
                                .unwrap_or_else(|| "-".into()),
                            file.path
                        );
                    }
                }
                FilesCmd::Get {
                    session_id,
                    path,
                    output,
                } => {
                    let bytes = c.download_file(&session_id, &path).await?;
                    std::fs::write(&output, &bytes)?;
                    println!("wrote {} bytes to {}", bytes.len(), output.display());
                }
                FilesCmd::Put {
                    session_id,
                    path,
                    input,
                } => {
                    let bytes = std::fs::read(&input)?;
                    let entry = c.upload_file(&session_id, &path, bytes).await?;
                    println!(
                        "uploaded {} bytes to {}",
                        entry.size.unwrap_or_default(),
                        entry.path
                    );
                }
            },
            SessionCmd::Persist {
                session_id,
                name,
                path,
                media_type,
            } => {
                let artifact = c
                    .persist_artifact(&session_id, &name, &path, media_type.as_deref())
                    .await?;
                println!(
                    "artifact {}  {} bytes  sha256 {}",
                    artifact.name, artifact.bytes, *artifact.sha256
                );
            }
            SessionCmd::Artifacts { cmd } => match cmd {
                ArtifactsCmd::List { session_id } => {
                    for artifact in c.list_artifacts(&session_id).await?.data {
                        println!(
                            "{:<32} {:>12}  {}",
                            artifact.name, artifact.bytes, *artifact.sha256
                        );
                    }
                }
                ArtifactsCmd::Get { session_id, name } => {
                    let artifact = c.get_artifact(&session_id, &name).await?;
                    println!("{}", serde_json::to_string_pretty(&artifact)?);
                }
            },
            SessionCmd::Delete { session_id } => {
                c.delete_session(&session_id).await?;
                println!("deleted {session_id} (irreversible)");
            }
        },
    }
    Ok(())
}

/// Render one event; returns false when the awaited turn is over.
fn render_event(ev: &Event, turn: &aex_contracts::session::TurnId, saw_delta: &mut bool) -> bool {
    match ev {
        Event::TurnStarted { .. } => true,
        Event::AssistantDelta { text, .. } => {
            print!("{text}");
            std::io::stdout().flush().ok();
            *saw_delta = true;
            true
        }
        Event::AssistantMessage { text, .. } => {
            if *saw_delta {
                println!();
                *saw_delta = false;
            } else {
                println!("{text}");
            }
            true
        }
        Event::ToolCall { name, input, .. } => {
            println!("→ {name} {}", compact(input, 120));
            true
        }
        Event::ToolOutput { text, .. } => {
            print!("{text}");
            std::io::stdout().flush().ok();
            true
        }
        Event::ToolResult {
            name,
            outcome,
            exit_code,
            duration_ms,
            ..
        } => {
            println!(
                "← {name}: {outcome}{} ({duration_ms} ms)",
                exit_code.map(|c| format!(" exit={c}")).unwrap_or_default()
            );
            true
        }
        Event::ModelUsage { .. } | Event::SessionUpdated { .. } => true,
        Event::AgentSpawned {
            agent_id,
            description,
            ..
        } => {
            println!("⇒ subagent {}: {description}", **agent_id);
            true
        }
        Event::AgentFinished {
            agent_id, outcome, ..
        } => {
            println!("⇐ subagent {}: {outcome}", **agent_id);
            true
        }
        Event::HandLost { .. } => {
            println!("! hand lost (workspace restores from the last sync)");
            true
        }
        Event::TurnCompleted {
            turn_id,
            rounds,
            tool_calls,
            result,
            ..
        } => {
            if let Some(result) = result {
                println!(
                    "✓ {} {}",
                    result.name,
                    serde_json::to_string(&result.value).unwrap_or_default()
                );
            }
            println!("✓ turn completed ({rounds} rounds, {tool_calls} tool calls)");
            turn_id != turn
        }
        Event::TurnFailed { turn_id, error, .. } => {
            println!(
                "✗ turn failed: {}",
                serde_json::to_string(error).unwrap_or_default()
            );
            turn_id != turn
        }
    }
}

fn compact(v: &Value, max: usize) -> String {
    let s = v.to_string();
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
