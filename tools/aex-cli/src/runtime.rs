//! Production CLI execution over the generated wire client.

use std::collections::BTreeMap;
use std::io::{IsTerminal as _, Read as _, Write as _};
use std::path::Path;
use std::sync::Arc;

use aex_wire::CanonicalJson;
use aex_wire::client::{
    BaseUrl, ClientError, Transport, TransportError, WireClient, WireRequest, WireResponse,
};
use aex_wire::cursor::Cursor;
use aex_wire::error::ErrorClass;
use aex_wire::idempotency::IdempotencyKey;
use aex_wire::ids::{
    ContentHash, FilePath, OperationId, PaymentMethodId, PrefixedId as _, ResourceName, SessionId,
    Uuid7,
};
use aex_wire::models::{
    BillingTransactionsListQuery, BillingUsageCategory, BillingUsageGetQuery, BlobEncoding,
    BlobInline, BlobInput, BlobUrl, EmptyRequest, McpServer, MessageSendRequest,
    PaymentMethodSessionRequest, ProviderId, RegisteredFileMode, RegisteredFileValue,
    RegistryDownloadRequest, ResponseFormat, ResponseFormatKind, SessionCreateRequest,
    SessionMessagesListQuery, SessionMessagesStreamQuery, SessionRegisteredSelection,
    SessionSandboxRequest, SessionStatus, SessionTelemetryReplayQuery, SessionTelemetryStreamQuery,
    SessionsListQuery, TelemetryDownloadRequest, TopUpCheckoutRequest, UploadCompleteRequest,
    UploadCreateRequest, UploadPart, UploadPartRequest, WorkspaceFileMount,
};
use aex_wire::types::{Cents, DecimalU128, ETag, HttpsUrl, Timestamp};
use base64::Engine as _;
use futures::StreamExt as _;
use reqwest::header::{AUTHORIZATION, ETAG};
use serde::Serialize;
use sha2::Digest as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use zeroize::{Zeroize as _, Zeroizing};

use crate::cli::{
    BillingCommand, BillingUsageArgs, Cli, Command, FileCommand, MessageCommand, SessionCommand,
    SessionCreateArgs, TelemetryCommand,
};
use crate::config::{
    ConfigInputs, Profile, config_path, get_value, load_profile, resolve_api_key, resolve_config,
    resolve_dashboard_session, set_value, unset_value,
};
use crate::download::prepare_download;
use crate::error::{CliErrorClass, exit_code_for_error_class};
use crate::output::OutputFormat;

const INLINE_ENCODED_MAX: usize = 32_768;
const UPLOAD_PART_BYTES: usize = 8 * 1024 * 1024;

/// A customer-safe process error with its stable exit status.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct RuntimeError {
    message: String,
    exit: u8,
}

impl RuntimeError {
    /// Stable process exit status.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        self.exit
    }

    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit: CliErrorClass::Usage.exit_code(),
        }
    }

    fn configuration(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit: CliErrorClass::Configuration.exit_code(),
        }
    }

    fn local_io(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit: CliErrorClass::LocalIo.exit_code(),
        }
    }

    fn interrupted() -> Self {
        Self {
            message: "interrupted".to_owned(),
            exit: CliErrorClass::Interrupted.exit_code(),
        }
    }
}

/// Converts a parser diagnostic into the stable usage exit class.
#[must_use]
pub fn parse_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::usage(message)
}

/// Customer-safe stdout failure.
#[must_use]
pub fn output_error() -> RuntimeError {
    RuntimeError::local_io("output failed")
}

impl From<ClientError> for RuntimeError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Api { error, route, .. } => {
                let exit = match error.code {
                    aex_wire::error::ObservedErrorCode::Known(code) => {
                        exit_code_for_error_class(code.class())
                    }
                    aex_wire::error::ObservedErrorCode::Unrecognized(_) => {
                        CliErrorClass::Unclassified.exit_code()
                    }
                };
                Self {
                    message: format!(
                        "{}: {}",
                        aex_wire::routes::route(route).operation_id,
                        error.message
                    ),
                    exit,
                }
            }
            ClientError::Transport(_) => Self {
                message: "the API request failed before a response was received".to_owned(),
                exit: exit_code_for_error_class(ErrorClass::Unavailable),
            },
            ClientError::Decode { route, .. } => Self {
                message: format!(
                    "{} returned an invalid response",
                    aex_wire::routes::route(route).operation_id
                ),
                exit: exit_code_for_error_class(ErrorClass::Internal),
            },
            ClientError::Encode { route, reason } => Self::usage(format!(
                "{} request is invalid: {reason}",
                aex_wire::routes::route(route).operation_id
            )),
        }
    }
}

/// Authenticated HTTP transport. Debug output is intentionally redacted.
#[derive(Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    base: BaseUrl,
    api_key: Arc<Zeroizing<String>>,
}

impl core::fmt::Debug for HttpTransport {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("HttpTransport")
            .field("base", &self.base)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl HttpTransport {
    fn new(base: BaseUrl, api_key: Arc<Zeroizing<String>>) -> Result<Self, RuntimeError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                RuntimeError::configuration("the HTTPS client could not be initialized")
            })?;
        Ok(Self {
            client,
            base,
            api_key,
        })
    }

    fn request(&self, request: &WireRequest) -> Result<reqwest::RequestBuilder, TransportError> {
        let method = reqwest::Method::from_bytes(request.method.as_str().as_bytes())
            .map_err(|_| TransportError::new("invalid generated method"))?;
        let mut builder = self
            .client
            .request(method, request.url(&self.base))
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key.as_str()));
        for (name, value) in &request.headers {
            builder = builder.header(*name, value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        Ok(builder)
    }

    async fn streaming_response(
        &self,
        request: WireRequest,
    ) -> Result<reqwest::Response, RuntimeError> {
        self.request(&request)
            .map_err(ClientError::from)?
            .send()
            .await
            .map_err(|_| ClientError::from(TransportError::new("request failed")))
            .map_err(RuntimeError::from)
    }
}

impl Transport for HttpTransport {
    fn execute(
        &self,
        request: WireRequest,
    ) -> impl core::future::Future<Output = Result<WireResponse, TransportError>> + Send {
        let this = self.clone();
        async move {
            let response = this
                .request(&request)?
                .send()
                .await
                .map_err(|_| TransportError::new("request failed"))?;
            let status = response.status().as_u16();
            let etag = response
                .headers()
                .get(ETAG)
                .and_then(|value| value.to_str().ok())
                .map(ETag::parse)
                .transpose()
                .map_err(|_| TransportError::new("invalid response ETag"))?;
            let body = response
                .bytes()
                .await
                .map_err(|_| TransportError::new("response body failed"))?
                .to_vec();
            Ok(WireResponse { status, etag, body })
        }
    }
}

struct Clients {
    central: WireClient<HttpTransport>,
    regional: WireClient<HttpTransport>,
    regional_transport: HttpTransport,
}

/// Executes one parsed invocation.
pub async fn execute(cli: Cli) -> Result<(), RuntimeError> {
    if matches!(cli.command, Some(Command::Version)) {
        println!(
            "aex {} target={} contract=d43caa52c2b1",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::ARCH
        );
        return Ok(());
    }
    let config_file = config_path(cli.config.as_deref())
        .map_err(|error| RuntimeError::configuration(error.to_string()))?;
    if let Some(Command::Config { command }) = &cli.command {
        return execute_config(command, &config_file, &cli.profile, cli.output);
    }
    let profile = load_profile(&config_file, &cli.profile)
        .map_err(|error| RuntimeError::configuration(error.to_string()))?;
    let public_env = public_environment();
    let resolved = resolve_config(ConfigInputs {
        central_url: cli.central_url.clone(),
        regional_url: cli.regional_url.clone(),
        output: cli.output,
        profile: profile.clone(),
        env: public_env,
        stdout_is_terminal: std::io::stdout().is_terminal(),
    })
    .map_err(|error| RuntimeError::configuration(error.to_string()))?;
    let command = cli
        .command
        .as_ref()
        .ok_or_else(|| RuntimeError::usage("a command is required"))?;
    let plane = credential_plane(command);
    let secret_env = credential_environment(&profile, plane);
    let credential = Arc::new(
        resolve_plane_credential(&profile, &secret_env, plane)
            .map_err(|error| RuntimeError::configuration(error.to_string()))?,
    );
    let central_base = BaseUrl::parse(&resolved.central_url)
        .map_err(|error| RuntimeError::configuration(error.to_string()))?;
    let regional_base = BaseUrl::parse(&resolved.regional_url)
        .map_err(|error| RuntimeError::configuration(error.to_string()))?;
    let central_transport = HttpTransport::new(central_base.clone(), Arc::clone(&credential))?;
    let regional_transport = HttpTransport::new(regional_base.clone(), credential)?;
    let clients = Clients {
        central: WireClient::new(central_transport.clone(), central_base),
        regional: WireClient::new(regional_transport.clone(), regional_base),
        regional_transport,
    };
    let key = replay_key(cli.idempotency_key.as_deref())?;
    match command {
        Command::Session { command } => {
            execute_session(command, &clients, &key, resolved.output).await
        }
        Command::Message { command } => {
            execute_message(command, &clients, &key, resolved.output).await
        }
        Command::File { command } => execute_file(command, &clients, &key, resolved.output).await,
        Command::Telemetry { command } => {
            execute_telemetry(command, &clients, &key, resolved.output).await
        }
        Command::Billing { command } => {
            execute_billing(command, &clients, &key, resolved.output).await
        }
        Command::Config { .. } | Command::Version => Ok(()),
    }
}

fn execute_config(
    command: &crate::cli::ConfigCommand,
    path: &Path,
    profile_name: &str,
    output: Option<OutputFormat>,
) -> Result<(), RuntimeError> {
    use crate::cli::ConfigCommand;
    match command {
        ConfigCommand::Path => println!("{}", path.display()),
        ConfigCommand::List => {
            let profile = load_profile(path, profile_name)
                .map_err(|error| RuntimeError::configuration(error.to_string()))?;
            emit(&profile, output.unwrap_or(OutputFormat::Json))?;
        }
        ConfigCommand::Get { key } => {
            let profile = load_profile(path, profile_name)
                .map_err(|error| RuntimeError::configuration(error.to_string()))?;
            let value = get_value(&profile, key)
                .map_err(|error| RuntimeError::configuration(error.to_string()))?;
            emit(&value, output.unwrap_or(OutputFormat::Json))?;
        }
        ConfigCommand::Set { key, value } => set_value(path, profile_name, key, value)
            .map_err(|error| RuntimeError::configuration(error.to_string()))?,
        ConfigCommand::Unset { key } => unset_value(path, profile_name, key)
            .map_err(|error| RuntimeError::configuration(error.to_string()))?,
    }
    Ok(())
}

async fn execute_session(
    command: &SessionCommand,
    clients: &Clients,
    key: &IdempotencyKey,
    output: OutputFormat,
) -> Result<(), RuntimeError> {
    match command {
        SessionCommand::Create(args) => {
            let request = session_create_body(args)?;
            let session = clients.regional.session_create(&request, key).await?;
            emit(&session, output)
        }
        SessionCommand::List(args) => {
            let page = clients
                .regional
                .sessions_list(&SessionsListQuery {
                    cursor: cursor(args.page.cursor.as_deref())?,
                    limit: args.page.limit,
                    status: status(args.status.as_deref())?,
                })
                .await?;
            emit(&page, output)
        }
        SessionCommand::Get { session } => {
            let session = clients.regional.session_get(session_id(session)?).await?;
            emit(&session.value, output)
        }
        SessionCommand::Terminate { session } => {
            let receipt = clients
                .regional
                .session_terminate(session_id(session)?, &EmptyRequest {}, new_operation_id())
                .await?;
            emit(&receipt, output)
        }
        SessionCommand::Delete { session, .. } => {
            let receipt = clients
                .regional
                .session_delete(session_id(session)?, &EmptyRequest {}, new_operation_id())
                .await?;
            emit(&receipt, output)
        }
    }
}

fn session_create_body(args: &SessionCreateArgs) -> Result<SessionCreateRequest, RuntimeError> {
    if args.provider_key_stdin && args.mcp_json_stdin {
        return Err(RuntimeError::usage(
            "provider and MCP secrets cannot both consume stdin; use an environment variable for one",
        ));
    }
    let mut provider_key = if let Some(name) = &args.provider_key_env {
        read_secret_env(name, "provider key")?
    } else {
        read_secret_stdin("provider key")?
    };
    let mcp_servers = if let Some(name) = &args.mcp_json_env {
        let value = read_secret_env(name, "MCP configuration")?;
        let decoded = serde_json::from_str::<Vec<McpServer>>(value.as_str())
            .map_err(|_| RuntimeError::usage("MCP configuration is not a valid server array"))?;
        Some(decoded)
    } else if args.mcp_json_stdin {
        let value = read_secret_stdin("MCP configuration")?;
        let decoded = serde_json::from_str::<Vec<McpServer>>(value.as_str())
            .map_err(|_| RuntimeError::usage("MCP configuration is not a valid server array"))?;
        Some(decoded)
    } else {
        None
    };
    let mounts = args
        .mounts
        .iter()
        .map(|mount| {
            let (name, path) = mount
                .split_once('=')
                .ok_or_else(|| RuntimeError::usage("a mount must be NAME=/workspace/path"))?;
            Ok(WorkspaceFileMount {
                name: ResourceName::parse(name)
                    .map_err(|error| RuntimeError::usage(error.to_string()))?,
                path: FilePath::parse(path)
                    .map_err(|error| RuntimeError::usage(error.to_string()))?,
            })
        })
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    let body = SessionCreateRequest {
        expires_at: None,
        max_spend_cents: args.max_spend_cents.map(Cents::new),
        mcp_servers,
        metadata: None,
        model: args.model.clone(),
        provider: provider(&args.provider)?,
        provider_api_key: provider_key.as_str().to_owned(),
        registered: Some(SessionRegisteredSelection { mounts }),
        sandbox: Some(SessionSandboxRequest {
            compute: None,
            enabled: Some(!args.no_sandbox),
            network: None,
            packages: None,
        }),
    };
    provider_key.zeroize();
    Ok(body)
}

async fn execute_message(
    command: &MessageCommand,
    clients: &Clients,
    key: &IdempotencyKey,
    output: OutputFormat,
) -> Result<(), RuntimeError> {
    match command {
        MessageCommand::Send {
            session,
            text,
            max_spend_cents,
            response_schema,
        } => {
            let text = text.clone().map_or_else(read_text_stdin, Ok)?;
            let response_format = response_schema
                .as_deref()
                .map(read_schema)
                .transpose()?
                .map(|schema| ResponseFormat {
                    kind: ResponseFormatKind::JsonSchema,
                    schema: Some(schema),
                });
            let result = clients
                .regional
                .session_message_send(
                    session_id(session)?,
                    &MessageSendRequest {
                        deadline: None,
                        max_spend_cents: max_spend_cents.map(Cents::new),
                        response_format,
                        text,
                    },
                    key,
                )
                .await?;
            emit(&result, output)
        }
        MessageCommand::List { session, page } => {
            let messages = clients
                .regional
                .session_messages_list(
                    session_id(session)?,
                    &SessionMessagesListQuery {
                        cursor: cursor(page.cursor.as_deref())?,
                        limit: page.limit,
                    },
                )
                .await?;
            emit(&messages, output)
        }
        MessageCommand::Tail { session, after } => {
            let request = aex_wire::client::session_messages_stream_request(
                session_id(session)?,
                &SessionMessagesStreamQuery {
                    after: after.map(DecimalU128::new),
                },
            )?;
            stream_ndjson(&clients.regional_transport, request).await
        }
    }
}

async fn execute_file(
    command: &FileCommand,
    clients: &Clients,
    key: &IdempotencyKey,
    output: OutputFormat,
) -> Result<(), RuntimeError> {
    match command {
        FileCommand::Put {
            name,
            path,
            url,
            media_type,
            executable,
        } => {
            let content = if let Some(url) = url {
                BlobInput::Url(BlobUrl {
                    url: HttpsUrl::parse(url)
                        .map_err(|error| RuntimeError::usage(error.to_string()))?,
                })
            } else {
                let bytes = match path {
                    Some(path) => std::fs::read(path)
                        .map_err(|_| RuntimeError::local_io("the inline file could not be read"))?,
                    None => read_bounded_stdin(INLINE_ENCODED_MAX)?,
                };
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                if data.len() > INLINE_ENCODED_MAX {
                    return Err(RuntimeError::usage(
                        "inline data exceeds 32768 encoded bytes; use `aex file upload`",
                    ));
                }
                BlobInput::Inline(BlobInline {
                    data,
                    encoding: BlobEncoding::Base64,
                    sha256: ContentHash::of(&bytes),
                })
            };
            let file = clients
                .regional
                .registry_files_put(
                    &resource_name(name)?,
                    &RegisteredFileValue {
                        content,
                        media_type: media_type.clone(),
                        mode: if *executable {
                            RegisteredFileMode::V0755
                        } else {
                            RegisteredFileMode::V0644
                        },
                    },
                    key,
                    None,
                )
                .await?;
            emit(&file.value, output)
        }
        FileCommand::Upload {
            name,
            path,
            media_type,
        } => upload_file(clients, name, path, media_type, key, output).await,
        FileCommand::List(page) => {
            let files = clients
                .regional
                .registry_files_list(&aex_wire::models::RegistryFilesListQuery {
                    cursor: cursor(page.cursor.as_deref())?,
                    limit: page.limit,
                })
                .await?;
            emit(&files, output)
        }
        FileCommand::Get { name } => {
            let file = clients
                .regional
                .registry_files_get(&resource_name(name)?)
                .await?;
            emit(&file.value, output)
        }
        FileCommand::Download {
            name,
            destination,
            force,
        } => {
            let grant = clients
                .regional
                .registry_files_download_create(
                    &resource_name(name)?,
                    &RegistryDownloadRequest { range: None },
                    key,
                )
                .await?;
            download_grant(
                &clients.regional_transport.client,
                grant.url.as_str(),
                destination,
                *force,
                grant.sha256,
                grant.size_bytes,
            )
            .await?;
            emit(
                &serde_json::json!({"name": name, "path": destination, "sha256": grant.sha256}),
                output,
            )
        }
        FileCommand::Delete { name } => {
            clients
                .regional
                .registry_files_delete(&resource_name(name)?, None)
                .await?;
            emit(&serde_json::json!({"name": name, "deleted": true}), output)
        }
    }
}

async fn upload_file(
    clients: &Clients,
    name: &str,
    path: &Path,
    media_type: &str,
    key: &IdempotencyKey,
    output: OutputFormat,
) -> Result<(), RuntimeError> {
    let (whole_hash, size, parts) = hash_file(path).await?;
    let admission = clients
        .regional
        .upload_create(
            &UploadCreateRequest {
                content_type: media_type.to_owned(),
                name: resource_name(name)?,
                parts,
                sha256: whole_hash,
                size_bytes: DecimalU128::new(size),
            },
            key,
        )
        .await?;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| RuntimeError::local_io("the upload file could not be reopened"))?;
    let mut completed = Vec::with_capacity(admission.grants.len());
    for grant in &admission.grants {
        let expected = admission
            .upload
            .part_size_bytes
            .get()
            .min(size.saturating_sub(
                u128::from(grant.part_number - 1) * admission.upload.part_size_bytes.get(),
            ));
        let expected = usize::try_from(expected)
            .map_err(|_| RuntimeError::local_io("an upload part exceeds this process"))?;
        let mut bytes = vec![0_u8; expected];
        file.read_exact(&mut bytes)
            .await
            .map_err(|_| RuntimeError::local_io("the upload file changed while uploading"))?;
        let mut request = clients
            .regional_transport
            .client
            .put(grant.url.as_str())
            .body(bytes);
        for header in &grant.headers {
            request = request.header(&header.name, &header.value);
        }
        let response = request
            .send()
            .await
            .map_err(|_| RuntimeError::local_io("an upload part could not be sent"))?;
        if !response.status().is_success() {
            return Err(RuntimeError::local_io("an upload part was rejected"));
        }
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| RuntimeError::local_io("an upload part returned no ETag"))?;
        completed.push(UploadPart {
            etag: ETag::parse(etag).map_err(|_| RuntimeError::local_io("invalid upload ETag"))?,
            part_number: grant.part_number,
        });
    }
    let completion_key = replay_key(None)?;
    let file = clients
        .regional
        .upload_complete(
            admission.upload.id,
            &UploadCompleteRequest { parts: completed },
            &completion_key,
        )
        .await?;
    emit(&file, output)
}

async fn hash_file(
    path: &Path,
) -> Result<(ContentHash, u128, Vec<UploadPartRequest>), RuntimeError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| RuntimeError::local_io("the upload file could not be read"))?;
    let mut whole = sha2::Sha256::new();
    let mut requests = Vec::new();
    let mut size = 0_u128;
    let mut part_number = 1_u32;
    loop {
        let mut part = vec![0_u8; UPLOAD_PART_BYTES];
        let mut read = 0_usize;
        while read < part.len() {
            let count = file
                .read(&mut part[read..])
                .await
                .map_err(|_| RuntimeError::local_io("the upload file could not be hashed"))?;
            if count == 0 {
                break;
            }
            read += count;
        }
        if read == 0 {
            break;
        }
        part.truncate(read);
        whole.update(&part);
        size = size
            .checked_add(read as u128)
            .ok_or_else(|| RuntimeError::local_io("the upload file is too large"))?;
        requests.push(UploadPartRequest {
            part_number,
            sha256: ContentHash::of(&part),
            size_bytes: DecimalU128::new(read as u128),
        });
        part_number = part_number
            .checked_add(1)
            .ok_or_else(|| RuntimeError::usage("the upload needs too many parts"))?;
        if requests.len() > 1_000 {
            return Err(RuntimeError::usage("the upload needs more than 1000 parts"));
        }
    }
    if requests.is_empty() {
        requests.push(UploadPartRequest {
            part_number: 1,
            sha256: ContentHash::of(&[]),
            size_bytes: DecimalU128::ZERO,
        });
    }
    Ok((
        ContentHash::from_bytes(whole.finalize().into()),
        size,
        requests,
    ))
}

async fn execute_telemetry(
    command: &TelemetryCommand,
    clients: &Clients,
    key: &IdempotencyKey,
    output: OutputFormat,
) -> Result<(), RuntimeError> {
    match command {
        TelemetryCommand::Tail { session, after } => {
            let request = aex_wire::client::session_telemetry_stream_request(
                session_id(session)?,
                &SessionTelemetryStreamQuery {
                    after: after.map(DecimalU128::new),
                },
            )?;
            stream_ndjson(&clients.regional_transport, request).await
        }
        TelemetryCommand::Replay {
            session,
            after,
            limit,
        } => {
            let replay = clients
                .regional
                .session_telemetry_replay(
                    session_id(session)?,
                    &SessionTelemetryReplayQuery {
                        after: after.map(DecimalU128::new),
                        limit: *limit,
                    },
                )
                .await?;
            std::io::stdout()
                .write_all(replay.as_bytes())
                .map_err(|_| RuntimeError::local_io("telemetry output failed"))?;
            Ok(())
        }
        TelemetryCommand::Download {
            session,
            destination,
            from_sequence,
            to_sequence,
            force,
        } => {
            let grant = clients
                .regional
                .session_telemetry_download_create(
                    session_id(session)?,
                    &TelemetryDownloadRequest {
                        from_sequence: from_sequence.map(DecimalU128::new),
                        to_sequence: to_sequence.map(DecimalU128::new),
                    },
                    key,
                )
                .await?;
            download_grant(
                &clients.regional_transport.client,
                grant.url.as_str(),
                destination,
                *force,
                grant.sha256,
                grant.size_bytes,
            )
            .await?;
            emit(
                &serde_json::json!({"session": session, "path": destination, "sha256": grant.sha256}),
                output,
            )
        }
    }
}

async fn execute_billing(
    command: &BillingCommand,
    clients: &Clients,
    key: &IdempotencyKey,
    output: OutputFormat,
) -> Result<(), RuntimeError> {
    match command {
        BillingCommand::Balance => emit(&clients.central.billing_balance_get().await?, output),
        BillingCommand::Cards => emit(
            &clients.central.billing_payment_methods_list().await?,
            output,
        ),
        BillingCommand::SetupCard {
            consent,
            success_url,
            cancel_url,
        } => {
            if !consent {
                return Err(RuntimeError::usage(
                    "--consent is required before opening a hosted card setup",
                ));
            }
            let hosted = clients
                .central
                .billing_payment_method_session_create(
                    &PaymentMethodSessionRequest {
                        cancel_url: https_url(cancel_url.as_deref())?,
                        consent: true,
                        success_url: https_url(success_url.as_deref())?,
                    },
                    key,
                )
                .await?;
            emit_hosted(&hosted, output)
        }
        BillingCommand::RemoveCard { payment_method } => {
            clients
                .central
                .billing_payment_method_delete(
                    PaymentMethodId::parse(payment_method)
                        .map_err(|error| RuntimeError::usage(error.to_string()))?,
                    key,
                )
                .await?;
            emit(
                &serde_json::json!({"paymentMethod": payment_method, "removed": true}),
                output,
            )
        }
        BillingCommand::Topup {
            amount_cents,
            success_url,
            cancel_url,
        } => {
            let hosted = clients
                .central
                .billing_top_up_checkout_create(
                    &TopUpCheckoutRequest {
                        amount_cents: Cents::new(*amount_cents),
                        cancel_url: https_url(cancel_url.as_deref())?,
                        success_url: https_url(success_url.as_deref())?,
                    },
                    key,
                )
                .await?;
            emit_hosted(&hosted, output)
        }
        BillingCommand::Transactions(page) => {
            let transactions = clients
                .central
                .billing_transactions_list(&BillingTransactionsListQuery {
                    cursor: cursor(page.cursor.as_deref())?,
                    limit: page.limit,
                })
                .await?;
            emit(&transactions, output)
        }
        BillingCommand::Usage(args) => {
            let usage = clients
                .central
                .billing_usage_get(&billing_usage_query(args)?)
                .await?;
            emit(&usage, output)
        }
    }
}

fn billing_usage_query(args: &BillingUsageArgs) -> Result<BillingUsageGetQuery, RuntimeError> {
    let category = match args.category.as_deref() {
        None => None,
        Some("model") => Some(BillingUsageCategory::Model),
        Some("runtime") => Some(BillingUsageCategory::Runtime),
        Some("storage") => Some(BillingUsageCategory::Storage),
        Some("transfer") => Some(BillingUsageCategory::Transfer),
        Some(_) => return Err(RuntimeError::usage("invalid usage category")),
    };
    Ok(BillingUsageGetQuery {
        category,
        cursor: cursor(args.page.cursor.as_deref())?,
        from: timestamp(args.from.as_deref())?,
        limit: args.page.limit,
        session_id: args.session.as_deref().map(session_id).transpose()?,
        to: timestamp(args.to.as_deref())?,
    })
}

async fn stream_ndjson(
    transport: &HttpTransport,
    request: WireRequest,
) -> Result<(), RuntimeError> {
    let expected = aex_wire::routes::route(request.route).success_status;
    let response = transport.streaming_response(request).await?;
    if response.status().as_u16() != expected {
        return Err(RuntimeError {
            message: format!(
                "stream request was rejected with HTTP {}",
                response.status()
            ),
            exit: if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
                exit_code_for_error_class(ErrorClass::Auth)
            } else {
                exit_code_for_error_class(ErrorClass::Unavailable)
            },
        });
    }
    let mut stream = response.bytes_stream();
    let mut stdout = tokio::io::stdout();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Err(RuntimeError::interrupted()),
            next = stream.next() => match next {
                Some(Ok(bytes)) => stdout.write_all(&bytes).await
                    .map_err(|_| RuntimeError::local_io("stream output failed"))?,
                Some(Err(_)) => return Err(RuntimeError {
                    message: "the live stream was interrupted; rerun with --after to reconcile".to_owned(),
                    exit: exit_code_for_error_class(ErrorClass::Unavailable),
                }),
                None => break,
            }
        }
    }
    stdout
        .flush()
        .await
        .map_err(|_| RuntimeError::local_io("stream output failed"))
}

async fn download_grant(
    client: &reqwest::Client,
    signed_url: &str,
    destination: &Path,
    force: bool,
    expected_hash: ContentHash,
    expected_size: DecimalU128,
) -> Result<(), RuntimeError> {
    let plan = prepare_download(destination, force, false)
        .map_err(|error| RuntimeError::local_io(error.to_string()))?;
    let response = client
        .get(signed_url)
        .send()
        .await
        .map_err(|_| RuntimeError::local_io("the signed download failed"))?;
    if !response.status().is_success() {
        return Err(RuntimeError::local_io("the signed download was rejected"));
    }
    let mut target = tokio::fs::File::create(&plan.part_path)
        .await
        .map_err(|_| RuntimeError::local_io("the download part file could not be created"))?;
    let mut stream = response.bytes_stream();
    let mut hash = sha2::Sha256::new();
    let mut size = 0_u128;
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| RuntimeError::local_io("the download body was interrupted"))?;
        size = size
            .checked_add(chunk.len() as u128)
            .ok_or_else(|| RuntimeError::local_io("the download is too large"))?;
        hash.update(&chunk);
        target
            .write_all(&chunk)
            .await
            .map_err(|_| RuntimeError::local_io("the download part file could not be written"))?;
    }
    target
        .flush()
        .await
        .map_err(|_| RuntimeError::local_io("the download part file could not be flushed"))?;
    drop(target);
    let observed_hash = ContentHash::from_bytes(hash.finalize().into());
    if size != expected_size.get() || observed_hash != expected_hash {
        let _ = tokio::fs::remove_file(&plan.part_path).await;
        return Err(RuntimeError::local_io(
            "the downloaded bytes failed length or SHA-256 verification",
        ));
    }
    if force && destination.exists() {
        tokio::fs::remove_file(destination).await.map_err(|_| {
            RuntimeError::local_io("the existing destination could not be replaced")
        })?;
    }
    tokio::fs::rename(&plan.part_path, destination)
        .await
        .map_err(|_| RuntimeError::local_io("the verified download could not be committed"))
}

fn emit(value: &impl Serialize, output: OutputFormat) -> Result<(), RuntimeError> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    match output {
        OutputFormat::Text => serde_json::to_writer_pretty(&mut lock, value),
        OutputFormat::Json | OutputFormat::Ndjson => serde_json::to_writer(&mut lock, value),
    }
    .map_err(|_| RuntimeError::local_io("output serialization failed"))?;
    writeln!(lock).map_err(|_| RuntimeError::local_io("output failed"))
}

fn emit_hosted(
    hosted: &aex_wire::models::HostedSession,
    output: OutputFormat,
) -> Result<(), RuntimeError> {
    if output == OutputFormat::Text {
        println!("{}", hosted.url);
        Ok(())
    } else {
        emit(hosted, output)
    }
}

fn public_environment() -> BTreeMap<String, String> {
    ["AEX_CENTRAL_URL", "AEX_REGIONAL_URL", "AEX_OUTPUT"]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_owned(), value))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialPlane {
    Central,
    Regional,
}

const fn credential_plane(command: &Command) -> CredentialPlane {
    if matches!(command, Command::Billing { .. }) {
        CredentialPlane::Central
    } else {
        CredentialPlane::Regional
    }
}

fn resolve_plane_credential(
    profile: &Profile,
    env: &BTreeMap<String, String>,
    plane: CredentialPlane,
) -> Result<Zeroizing<String>, crate::config::CliConfigError> {
    match plane {
        CredentialPlane::Central => resolve_dashboard_session(profile, env),
        CredentialPlane::Regional => resolve_api_key(profile, env),
    }
}

fn credential_environment(profile: &Profile, plane: CredentialPlane) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let (ambient, reference) = match plane {
        CredentialPlane::Central => (
            "AEX_DASHBOARD_SESSION",
            profile.dashboard_session_ref.as_deref(),
        ),
        CredentialPlane::Regional => ("AEX_API_KEY", profile.api_key_ref.as_deref()),
    };
    if let Ok(value) = std::env::var(ambient) {
        values.insert(ambient.to_owned(), value);
    }
    if let Some(name) = reference.and_then(|value| value.strip_prefix("env:"))
        && let Ok(value) = std::env::var(name)
    {
        values.insert(name.to_owned(), value);
    }
    values
}

fn replay_key(value: Option<&str>) -> Result<IdempotencyKey, RuntimeError> {
    let value = value
        .map(str::to_owned)
        .unwrap_or_else(|| format!("cli-{}", uuid::Uuid::now_v7()));
    IdempotencyKey::parse(&value).map_err(|error| RuntimeError::usage(error.to_string()))
}

fn new_operation_id() -> OperationId {
    let uuid = uuid::Uuid::now_v7();
    OperationId::from_uuid7(
        Uuid7::from_bytes(*uuid.as_bytes()).expect("uuid crate produced UUIDv7"),
    )
}

fn resource_name(value: &str) -> Result<ResourceName, RuntimeError> {
    ResourceName::parse(value).map_err(|error| RuntimeError::usage(error.to_string()))
}

fn session_id(value: &str) -> Result<SessionId, RuntimeError> {
    SessionId::parse(value).map_err(|error| RuntimeError::usage(error.to_string()))
}

fn cursor(value: Option<&str>) -> Result<Option<Cursor>, RuntimeError> {
    value
        .map(Cursor::parse)
        .transpose()
        .map_err(|error| RuntimeError::usage(error.to_string()))
}

fn provider(value: &str) -> Result<ProviderId, RuntimeError> {
    match value {
        "openai" => Ok(ProviderId::Openai),
        "anthropic" => Ok(ProviderId::Anthropic),
        "deepseek" => Ok(ProviderId::Deepseek),
        "xai" => Ok(ProviderId::Xai),
        "meta" => Ok(ProviderId::Meta),
        "moonshotai" => Ok(ProviderId::Moonshotai),
        "alibaba" => Ok(ProviderId::Alibaba),
        _ => Err(RuntimeError::usage("unsupported provider")),
    }
}

fn status(value: Option<&str>) -> Result<Option<SessionStatus>, RuntimeError> {
    match value {
        None => Ok(None),
        Some("idle") => Ok(Some(SessionStatus::Idle)),
        Some("running") => Ok(Some(SessionStatus::Running)),
        Some("terminating") => Ok(Some(SessionStatus::Terminating)),
        Some("terminated") => Ok(Some(SessionStatus::Terminated)),
        Some("deleting") => Ok(Some(SessionStatus::Deleting)),
        Some(_) => Err(RuntimeError::usage("invalid session status")),
    }
}

fn timestamp(value: Option<&str>) -> Result<Option<Timestamp>, RuntimeError> {
    value
        .map(Timestamp::parse)
        .transpose()
        .map_err(|error| RuntimeError::usage(error.to_string()))
}

fn https_url(value: Option<&str>) -> Result<Option<HttpsUrl>, RuntimeError> {
    value
        .map(HttpsUrl::parse)
        .transpose()
        .map_err(|error| RuntimeError::usage(error.to_string()))
}

fn read_secret_env(name: &str, kind: &str) -> Result<Zeroizing<String>, RuntimeError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(RuntimeError::usage(format!(
            "{kind} environment variable name is invalid"
        )));
    }
    let value = std::env::var(name).map_err(|_| {
        RuntimeError::configuration(format!("{kind} environment variable is unset"))
    })?;
    if value.is_empty() {
        return Err(RuntimeError::configuration(format!("{kind} is empty")));
    }
    Ok(Zeroizing::new(value))
}

fn read_secret_stdin(kind: &str) -> Result<Zeroizing<String>, RuntimeError> {
    if std::io::stdin().is_terminal() {
        return Err(RuntimeError::configuration(format!(
            "{kind} stdin source requires piped input; interactive echo is refused"
        )));
    }
    let mut value = String::new();
    std::io::stdin()
        .read_to_string(&mut value)
        .map_err(|_| RuntimeError::local_io(format!("{kind} could not be read")))?;
    let trimmed = value.trim_end_matches(['\r', '\n']).to_owned();
    value.zeroize();
    if trimmed.is_empty() {
        return Err(RuntimeError::configuration(format!("{kind} is empty")));
    }
    Ok(Zeroizing::new(trimmed))
}

fn read_text_stdin() -> Result<String, RuntimeError> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|_| RuntimeError::local_io("message text could not be read"))?;
    if text.is_empty() {
        return Err(RuntimeError::usage("message text is empty"));
    }
    Ok(text)
}

fn read_bounded_stdin(max: usize) -> Result<Vec<u8>, RuntimeError> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| RuntimeError::local_io("inline stdin could not be read"))?;
    if bytes.len() > max {
        return Err(RuntimeError::usage(
            "inline stdin is too large; use `aex file upload`",
        ));
    }
    Ok(bytes)
}

fn read_schema(path: &Path) -> Result<CanonicalJson, RuntimeError> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| RuntimeError::local_io("response schema could not be read"))?;
    CanonicalJson::parse(&text).map_err(|_| RuntimeError::usage("response schema is invalid JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_plane_selects_the_exact_credential_and_missing_central_fails_before_http() {
        let profile = Profile {
            api_key_ref: Some("env:WORKSPACE_KEY".to_owned()),
            dashboard_session_ref: Some("env:DASHBOARD_SESSION".to_owned()),
            ..Profile::default()
        };
        let env = BTreeMap::from([
            ("WORKSPACE_KEY".to_owned(), "aex_wk_regional".to_owned()),
            ("DASHBOARD_SESSION".to_owned(), "aex_ds_central".to_owned()),
        ]);
        let billing = Command::Billing {
            command: BillingCommand::Balance,
        };
        assert_eq!(credential_plane(&billing), CredentialPlane::Central);
        assert_eq!(
            resolve_plane_credential(&profile, &env, credential_plane(&billing))
                .expect("central credential")
                .as_str(),
            "aex_ds_central"
        );
        let regional = Command::Session {
            command: SessionCommand::Get {
                session: "ses_0000000000e0081040g2081040".to_owned(),
            },
        };
        assert_eq!(credential_plane(&regional), CredentialPlane::Regional);
        assert_eq!(
            resolve_plane_credential(&profile, &env, credential_plane(&regional))
                .expect("regional credential")
                .as_str(),
            "aex_wk_regional"
        );
        let missing = BTreeMap::from([("WORKSPACE_KEY".to_owned(), "aex_wk_regional".to_owned())]);
        assert!(
            resolve_plane_credential(&profile, &missing, CredentialPlane::Central).is_err(),
            "billing must fail before building or sending an HTTP request"
        );
    }

    #[test]
    fn transport_debug_and_authorization_never_cross_plane_or_disclose_material() {
        let central_secret = Arc::new(Zeroizing::new("aex_ds_central_secret".to_owned()));
        let regional_secret = Arc::new(Zeroizing::new("aex_wk_regional_secret".to_owned()));
        let base = BaseUrl::parse("https://api.example").expect("base");
        let central = HttpTransport::new(base.clone(), central_secret).expect("central");
        let regional = HttpTransport::new(base, regional_secret).expect("regional");
        let request = aex_wire::client::billing_balance_get_request().expect("request");
        let central_request = central
            .request(&request)
            .expect("builder")
            .build()
            .expect("HTTP request");
        let regional_request = regional
            .request(&request)
            .expect("builder")
            .build()
            .expect("HTTP request");
        assert_eq!(
            central_request.headers()[AUTHORIZATION],
            "Bearer aex_ds_central_secret"
        );
        assert_eq!(
            regional_request.headers()[AUTHORIZATION],
            "Bearer aex_wk_regional_secret"
        );
        let debug = format!("{central:?} {regional:?}");
        assert!(!debug.contains("central_secret"));
        assert!(!debug.contains("regional_secret"));
    }
}
