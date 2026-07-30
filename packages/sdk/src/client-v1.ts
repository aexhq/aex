import {
  CredentialValidationError,
  HTTP_RETRY_POLICY,
  HttpClient,
  assertId,
  newId,
  parseApiKey,
  RegisteredNameSchema,
  type Approval,
  type ApprovalResponseRequest,
  type DebugSink,
  type DownloadGrant,
  type FileDownloadRequest,
  type FileEntry,
  type FetchLike,
  type HttpRetryOptions,
  type Id,
  type LiveDownloadGrant,
  type LiveFileDownloadRequest,
  type LiveFileEntry,
  type LiveFileListRequest,
  type LiveFilePage,
  type LiveFileStatRequest,
  type MessageSendRequest,
  type MessageV1,
  type Operation,
  type OperationKind,
  type OperationStatusV1,
  type Page,
  type PersistedFileListRequest,
  type PersistedFileStatRequest,
  type RegisteredResource,
  type RegisteredResourceKind,
  type RegisteredResourceSummary,
  type RunStatusV1,
  type RunV1,
  type SecretMetadataV1,
  type SecretRevocation,
  type SessionCreateRequestV1,
  type SessionStatusV1,
  type SessionV1,
  type Upload,
  type UploadCompleteRequest,
  type UploadCreateRequest,
  type UploadPartsRequest,
  type UploadPartsResponse
} from "@aexhq/contracts";

export interface AexOptions {
  readonly apiKey?: string;
  readonly baseUrl?: string;
  readonly fetch?: FetchLike;
  readonly debug?: boolean | DebugSink;
  readonly retry?: HttpRetryOptions | false;
}

export interface IdempotencyOptions {
  readonly idempotencyKey?: string;
}

export interface OperationAdmissionOptions {
  readonly operationId?: Id<"operation">;
}

export interface RevisionOperationAdmissionOptions extends OperationAdmissionOptions {
  readonly ifRevision?: number;
}

export interface RevisionOptions {
  readonly ifRevision?: number;
}

export interface RevisionMutationOptions extends IdempotencyOptions, RevisionOptions {}

export interface WaitOptions {
  readonly pollIntervalMs?: number;
  readonly timeoutMs?: number;
  readonly signal?: AbortSignal;
}

export interface SessionListQuery {
  readonly status?: SessionStatusV1;
  readonly cursor?: string;
  readonly limit?: number;
}

export interface OperationListQuery {
  readonly sessionId?: string;
  readonly kind?: OperationKind;
  readonly status?: OperationStatusV1;
  readonly cursor?: string;
  readonly limit?: number;
}

export interface SessionPersistRequest {
  readonly include?: readonly string[];
  readonly exclude?: readonly string[];
}

export interface SessionForkRequest {
  readonly files: "current" | "initial" | "none";
  readonly credentials: "copy" | "none";
}

export interface WorkspaceDiscardRequest {
  readonly ifGenerationId?: string;
}

export interface CredentialRebindRequest {
  readonly secrets: readonly { readonly name: string }[];
}

export interface SessionDeleteRequest {
  readonly cascade: boolean;
}

export interface MessageSendAccepted {
  readonly message: MessageV1;
  readonly run: RunHandle;
}

export type MessagePart = MessageSendRequest["content"][number];

type OperationFor<K extends OperationKind> = Extract<Operation, { readonly kind: K }>;
export type OperationResult<K extends OperationKind> =
  NonNullable<OperationFor<K>["result"]>;

const TERMINAL_RUN_STATUSES: ReadonlySet<RunStatusV1> = new Set([
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted"
]);

const TERMINAL_OPERATION_STATUSES: ReadonlySet<OperationStatusV1> = new Set([
  "succeeded",
  "failed",
  "cancelled"
]);

export class RunFailedError extends Error {
  readonly run: RunV1;

  constructor(run: RunV1) {
    super(run.error?.message ?? `Run ${run.id} ended ${run.status}`);
    this.name = "RunFailedError";
    this.run = run;
  }
}

export class OperationFailedError extends Error {
  readonly operation: Operation;
  readonly operationId: string;

  constructor(operation: Operation) {
    super(operation.error?.message ?? `Operation ${operation.id} ended ${operation.status}`);
    this.name = "OperationFailedError";
    this.operation = operation;
    this.operationId = operation.id;
  }
}

export class RunHandle {
  readonly #http: HttpClient;
  #run: RunV1;

  constructor(http: HttpClient, run: RunV1) {
    this.#http = http;
    assertId("session", run.sessionId, "run.sessionId");
    assertId("run", run.id, "run.id");
    this.#run = run;
  }

  get id(): string {
    return this.#run.id;
  }

  get record(): RunV1 {
    return this.#run;
  }

  get status(): RunStatusV1 {
    return this.#run.status;
  }

  async get(): Promise<RunV1> {
    this.#run = this.#checked(
      await getRun(this.#http, this.#run.sessionId, this.id)
    );
    return this.#run;
  }

  async wait(options: WaitOptions = {}): Promise<RunV1> {
    this.#run = await pollUntilTerminal(
      this.#run,
      (value) => TERMINAL_RUN_STATUSES.has(value.status),
      async () => this.#checked(
        await getRun(this.#http, this.#run.sessionId, this.id)
      ),
      options
    );
    return this.#run;
  }

  async result(options: WaitOptions = {}): Promise<RunV1> {
    const terminal = await this.wait(options);
    if (terminal.status !== "succeeded") {
      throw new RunFailedError(terminal);
    }
    return terminal;
  }

  #checked(run: RunV1): RunV1 {
    if (run.id !== this.id || run.sessionId !== this.#run.sessionId) {
      throw new Error(
        `Run GET for ${this.#run.sessionId}/${this.id} returned ${run.sessionId}/${run.id}`
      );
    }
    return run;
  }
}

export class OperationHandle<K extends OperationKind = OperationKind> {
  readonly #http: HttpClient;
  #operation: OperationFor<K>;

  constructor(http: HttpClient, operation: OperationFor<K>) {
    this.#http = http;
    assertId("operation", operation.id, "operation.id");
    this.#operation = operation;
  }

  get id(): string {
    return this.#operation.id;
  }

  get kind(): K {
    return this.#operation.kind as K;
  }

  get status(): OperationStatusV1 {
    return this.#operation.status;
  }

  get record(): OperationFor<K> {
    return this.#operation;
  }

  async get(): Promise<OperationFor<K>> {
    this.#operation = this.#checked(
      await getOperation(this.#http, this.id) as OperationFor<K>
    );
    return this.#operation;
  }

  async wait(options: WaitOptions = {}): Promise<OperationFor<K>> {
    this.#operation = await pollUntilTerminal(
      this.#operation,
      (value) => TERMINAL_OPERATION_STATUSES.has(value.status),
      async () => this.#checked(
        await getOperation(this.#http, this.id) as OperationFor<K>
      ),
      options
    );
    return this.#operation;
  }

  async result(options: WaitOptions = {}): Promise<OperationResult<K>> {
    const terminal = await this.wait(options);
    if (terminal.status !== "succeeded") {
      throw new OperationFailedError(terminal);
    }
    if (terminal.result === undefined) {
      throw new Error(`Operation ${terminal.id} succeeded without a result`);
    }
    return terminal.result as OperationResult<K>;
  }

  #checked(operation: OperationFor<K>): OperationFor<K> {
    if (operation.kind !== this.kind) {
      throw new Error(
        `Operation ${this.id} changed kind from ${this.kind} to ${operation.kind}`
      );
    }
    if (operation.id !== this.id) {
      throw new Error(
        `Operation GET for ${this.id} returned ${operation.id}`
      );
    }
    return operation;
  }
}

export class SessionMessagesClient {
  readonly #http: HttpClient;
  readonly #sessionId: string;

  constructor(http: HttpClient, sessionId: string) {
    this.#http = http;
    this.#sessionId = sessionId;
  }

  async send(
    input: string | MessageSendRequest,
    options: IdempotencyOptions = {}
  ): Promise<MessageSendAccepted> {
    const request: MessageSendRequest = typeof input === "string"
      ? { content: [{ type: "text", text: input }] }
      : input;
    const accepted = await this.#http.request<{
      readonly message: MessageV1;
      readonly run: RunV1;
    }>(`/api/sessions/${encodeURIComponent(this.#sessionId)}/messages`, {
      method: "POST",
      headers: idempotencyHeaders(options),
      body: JSON.stringify(request)
    });
    return {
      message: accepted.message,
      run: new RunHandle(this.#http, accepted.run)
    };
  }
}

export class SessionWorkspaceClient {
  readonly #session: SessionHandle;

  constructor(session: SessionHandle) {
    this.#session = session;
  }

  discard(
    request: WorkspaceDiscardRequest = {},
    options: OperationAdmissionOptions = {}
  ): Promise<OperationHandle<"workspace_discard">> {
    return this.#session.admitOperation(
      "workspace_discard",
      "workspace/discards",
      request,
      options
    );
  }
}

export class SessionCredentialsClient {
  readonly #session: SessionHandle;

  constructor(session: SessionHandle) {
    this.#session = session;
  }

  rebind(
    request: CredentialRebindRequest,
    options: OperationAdmissionOptions = {}
  ): Promise<OperationHandle<"credential_rebind">> {
    return this.#session.admitOperation(
      "credential_rebind",
      "credential-rebinds",
      request,
      options
    );
  }
}

export class PersistedSessionFilesClient {
  readonly #http: HttpClient;
  readonly #sessionId: string;

  constructor(http: HttpClient, sessionId: string) {
    this.#http = http;
    this.#sessionId = sessionId;
  }

  list(request: PersistedFileListRequest = {}): Promise<Page<FileEntry>> {
    return this.#post<Page<FileEntry>>("list", request);
  }

  stat(request: PersistedFileStatRequest): Promise<FileEntry> {
    return this.#post<FileEntry>("stat", request);
  }

  download(
    request: FileDownloadRequest,
    options: IdempotencyOptions = {}
  ): Promise<DownloadGrant> {
    assertByteRange(request.range);
    return this.#post<DownloadGrant>("downloads", request, idempotencyHeaders(options));
  }

  #post<T>(route: string, body: unknown, headers?: HeadersInit): Promise<T> {
    return this.#http.request<T>(
      `/api/sessions/${encodeURIComponent(this.#sessionId)}/files/persisted/${route}`,
      { method: "POST", ...(headers ? { headers } : {}), body: JSON.stringify(body) }
    );
  }
}

export class LiveSessionFilesClient {
  readonly #http: HttpClient;
  readonly #sessionId: string;

  constructor(http: HttpClient, sessionId: string) {
    this.#http = http;
    this.#sessionId = sessionId;
  }

  list(request: LiveFileListRequest = {}): Promise<LiveFilePage> {
    assertGeneration(request.ifGenerationId);
    return this.#post<LiveFilePage>("list", request);
  }

  stat(request: LiveFileStatRequest): Promise<LiveFileEntry> {
    assertGeneration(request.ifGenerationId);
    return this.#post<LiveFileEntry>("stat", request);
  }

  download(
    request: LiveFileDownloadRequest,
    options: IdempotencyOptions = {}
  ): Promise<LiveDownloadGrant> {
    assertGeneration(request.ifGenerationId);
    assertByteRange(request.range);
    return this.#post<LiveDownloadGrant>("downloads", request, idempotencyHeaders(options));
  }

  #post<T>(route: string, body: unknown, headers?: HeadersInit): Promise<T> {
    return this.#http.request<T>(
      `/api/sessions/${encodeURIComponent(this.#sessionId)}/files/live/${route}`,
      { method: "POST", ...(headers ? { headers } : {}), body: JSON.stringify(body) }
    );
  }
}

export class SessionFilesClient {
  readonly persisted: PersistedSessionFilesClient;
  readonly live: LiveSessionFilesClient;

  constructor(http: HttpClient, sessionId: string) {
    this.persisted = new PersistedSessionFilesClient(http, sessionId);
    this.live = new LiveSessionFilesClient(http, sessionId);
  }
}

export interface ApprovalListQuery {
  readonly cursor?: string;
  readonly limit?: number;
}

export class SessionApprovalsClient {
  readonly #http: HttpClient;
  readonly #sessionId: string;

  constructor(http: HttpClient, sessionId: string) {
    this.#http = http;
    this.#sessionId = sessionId;
  }

  list(query: ApprovalListQuery = {}): Promise<Page<Approval>> {
    return this.#http.request<Page<Approval>>(
      `/api/sessions/${encodeURIComponent(this.#sessionId)}/approvals`,
      {},
      queryParameters(query)
    );
  }

  async get(approvalId: string): Promise<Approval> {
    const id = assertId("approval", approvalId, "approvalId");
    const approval = await this.#http.request<Approval>(
      `/api/sessions/${encodeURIComponent(this.#sessionId)}/approvals/${encodeURIComponent(id)}`
    );
    return this.#check(approval, id);
  }

  async respond(approvalId: string, request: ApprovalResponseRequest): Promise<Approval> {
    const id = assertId("approval", approvalId, "approvalId");
    const approval = await this.#http.request<Approval>(
      `/api/sessions/${encodeURIComponent(this.#sessionId)}/approvals/${encodeURIComponent(id)}/responses`,
      { method: "POST", body: JSON.stringify(request) }
    );
    return this.#check(approval, id);
  }

  #check(approval: Approval, id: string): Approval {
    if (approval.id !== id || approval.sessionId !== this.#sessionId) {
      throw new Error(
        `Approval ${this.#sessionId}/${id} returned ${approval.sessionId}/${approval.id}`
      );
    }
    return approval;
  }
}

export class SessionHandle {
  readonly #http: HttpClient;
  readonly #session: SessionV1;
  readonly messages: SessionMessagesClient;
  readonly workspace: SessionWorkspaceClient;
  readonly credentials: SessionCredentialsClient;
  readonly files: SessionFilesClient;
  readonly approvals: SessionApprovalsClient;

  constructor(http: HttpClient, session: SessionV1) {
    this.#http = http;
    assertId("session", session.id, "session.id");
    this.#session = session;
    this.messages = new SessionMessagesClient(http, session.id);
    this.workspace = new SessionWorkspaceClient(this);
    this.credentials = new SessionCredentialsClient(this);
    this.files = new SessionFilesClient(http, session.id);
    this.approvals = new SessionApprovalsClient(http, session.id);
  }

  get id(): string {
    return this.#session.id;
  }

  get record(): SessionV1 {
    return this.#session;
  }

  stop(
    options: OperationAdmissionOptions = {}
  ): Promise<OperationHandle<"session_stop">> {
    return this.admitOperation("session_stop", "stops", {}, options);
  }

  persist(
    request: SessionPersistRequest = {},
    options: RevisionOperationAdmissionOptions = {}
  ): Promise<OperationHandle<"session_persist">> {
    return this.admitOperation("session_persist", "persists", request, options);
  }

  fork(
    request: SessionForkRequest,
    options: RevisionOperationAdmissionOptions = {}
  ): Promise<OperationHandle<"session_fork">> {
    return this.admitOperation("session_fork", "forks", request, options);
  }

  delete(
    request: SessionDeleteRequest,
    options: OperationAdmissionOptions = {}
  ): Promise<OperationHandle<"session_delete">> {
    return this.admitOperation("session_delete", "deletions", request, options);
  }

  async admitOperation<K extends OperationKind>(
    kind: K,
    route: string,
    body: unknown,
    options: RevisionOperationAdmissionOptions
  ): Promise<OperationHandle<K>> {
    const operationId = options.operationId === undefined
      ? newId("operation")
      : assertId("operation", options.operationId, "operationId");
    const headers: Record<string, string> = {
      "Aex-Operation-Id": operationId
    };
    if (options.ifRevision !== undefined) {
      headers["If-Match"] = revisionEtag(options.ifRevision);
    }
    const operation = await this.#http.request<OperationFor<K>>(
      `/api/sessions/${encodeURIComponent(this.id)}/${route}`,
      {
        method: "POST",
        headers,
        body: JSON.stringify(body)
      }
    );
    if (operation.kind !== kind) {
      throw new Error(
        `Operation ${operation.id} has kind ${operation.kind}; expected ${kind}`
      );
    }
    if (operation.id !== operationId) {
      throw new Error(
        `Operation admission for ${operationId} returned ${operation.id}`
      );
    }
    return new OperationHandle(this.#http, operation);
  }
}

export class SessionsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  async create(
    request: SessionCreateRequestV1,
    options: IdempotencyOptions = {}
  ): Promise<SessionHandle> {
    const session = await this.#http.request<SessionV1>("/api/sessions", {
      method: "POST",
      headers: idempotencyHeaders(options),
      body: JSON.stringify(request)
    });
    return new SessionHandle(this.#http, session);
  }

  async get(sessionId: string): Promise<SessionV1> {
    const id = assertId("session", sessionId, "sessionId");
    const session = await this.#http.request<SessionV1>(
      `/api/sessions/${encodeURIComponent(id)}`
    );
    if (session.id !== id) {
      throw new Error(`Session GET for ${id} returned ${session.id}`);
    }
    return session;
  }

  async open(sessionId: string): Promise<SessionHandle> {
    return new SessionHandle(this.#http, await this.get(sessionId));
  }

  list(query: SessionListQuery = {}): Promise<Page<SessionV1>> {
    return this.#http.request<Page<SessionV1>>(
      "/api/sessions",
      {},
      queryParameters(query)
    );
  }
}

export class OperationsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  async get(operationId: string): Promise<Operation> {
    const id = assertId("operation", operationId, "operationId");
    const operation = await getOperation(
      this.#http,
      id
    );
    if (operation.id !== id) {
      throw new Error(`Operation GET for ${id} returned ${operation.id}`);
    }
    return operation;
  }

  async open(operationId: string): Promise<OperationHandle> {
    return new OperationHandle(this.#http, await this.get(operationId));
  }

  list(query: OperationListQuery = {}): Promise<Page<Operation>> {
    return this.#http.request<Page<Operation>>(
      "/api/operations",
      {},
      queryParameters(query)
    );
  }

  async cancel(operationId: string): Promise<Operation> {
    const id = assertId("operation", operationId, "operationId");
    const operation = await this.#http.request<Operation>(
      `/api/operations/${encodeURIComponent(id)}/cancellations`,
      {
        method: "POST",
        body: JSON.stringify({})
      }
    );
    if (operation.id !== id) {
      throw new Error(`Operation cancellation for ${id} returned ${operation.id}`);
    }
    return operation;
  }
}

type RegisteredResourceFor<K extends RegisteredResourceKind> =
  Extract<RegisteredResource, { readonly kind: K }>;
type RegisteredSummaryFor<K extends RegisteredResourceKind> =
  Extract<RegisteredResourceSummary, { readonly kind: K }>;

const REGISTRY_PATHS = {
  file: "files",
  skill: "skills",
  tool: "tools",
  instruction: "instructions",
  mcp_server: "mcp-servers"
} as const satisfies Readonly<Record<RegisteredResourceKind, string>>;

export class RegisteredResourceClient<K extends RegisteredResourceKind> {
  readonly #http: HttpClient;
  readonly #kind: K;
  readonly #path: string;

  constructor(http: HttpClient, kind: K) {
    this.#http = http;
    this.#kind = kind;
    this.#path = REGISTRY_PATHS[kind];
  }

  list(query: { readonly cursor?: string; readonly limit?: number } = {}):
    Promise<Page<RegisteredSummaryFor<K>>> {
    return this.#http.request<Page<RegisteredSummaryFor<K>>>(
      `/api/workspace/${this.#path}`,
      {},
      queryParameters(query)
    );
  }

  async get(name: string): Promise<RegisteredResourceFor<K>> {
    const resource = await this.#http.request<RegisteredResourceFor<K>>(
      `/api/workspace/${this.#path}/${encodeURIComponent(assertRegisteredName(name))}`
    );
    return this.#check(resource, name);
  }

  async set(
    name: string,
    value: RegisteredResourceFor<K>["value"],
    options: RevisionMutationOptions = {}
  ): Promise<RegisteredResourceFor<K>> {
    const resource = await this.#http.request<RegisteredResourceFor<K>>(
      `/api/workspace/${this.#path}/${encodeURIComponent(assertRegisteredName(name))}`,
      {
        method: "PUT",
        headers: mutationHeaders(options),
        body: JSON.stringify(value)
      }
    );
    return this.#check(resource, name);
  }

  async delete(name: string, options: RevisionOptions = {}): Promise<void> {
    const headers = revisionHeaders(options);
    await this.#http.request<void>(
      `/api/workspace/${this.#path}/${encodeURIComponent(assertRegisteredName(name))}`,
      {
        method: "DELETE",
        ...(headers ? { headers } : {})
      }
    );
  }

  #check(resource: RegisteredResourceFor<K>, name: string): RegisteredResourceFor<K> {
    if (resource.kind !== this.#kind || resource.name !== name) {
      throw new Error(
        `Registry ${this.#kind}/${name} returned ${resource.kind}/${resource.name}`
      );
    }
    return resource;
  }
}

export class WorkspaceUploadsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  create(
    request: UploadCreateRequest,
    options: IdempotencyOptions = {}
  ): Promise<Upload> {
    return this.#http.request<Upload>("/api/workspace/uploads", {
      method: "POST",
      headers: idempotencyHeaders(options),
      body: JSON.stringify(request)
    });
  }

  parts(uploadId: string, request: UploadPartsRequest): Promise<UploadPartsResponse> {
    const id = assertId("upload", uploadId, "uploadId");
    return this.#http.request<UploadPartsResponse>(
      `/api/workspace/uploads/${encodeURIComponent(id)}/parts`,
      { method: "POST", body: JSON.stringify(request) }
    );
  }

  complete(
    uploadId: string,
    request: UploadCompleteRequest,
    options: IdempotencyOptions = {}
  ): Promise<Upload> {
    const id = assertId("upload", uploadId, "uploadId");
    return this.#http.request<Upload>(
      `/api/workspace/uploads/${encodeURIComponent(id)}/completion`,
      {
        method: "POST",
        headers: idempotencyHeaders(options),
        body: JSON.stringify(request)
      }
    );
  }

  async abort(uploadId: string): Promise<void> {
    const id = assertId("upload", uploadId, "uploadId");
    await this.#http.request<void>(
      `/api/workspace/uploads/${encodeURIComponent(id)}`,
      { method: "DELETE" }
    );
  }
}

export class WorkspaceSecretsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(query: { readonly cursor?: string; readonly limit?: number } = {}):
    Promise<Page<SecretMetadataV1>> {
    return this.#http.request<Page<SecretMetadataV1>>(
      "/api/workspace/secrets",
      {},
      queryParameters(query)
    );
  }

  get(name: string): Promise<SecretMetadataV1> {
    return this.#http.request<SecretMetadataV1>(
      `/api/workspace/secrets/${encodeURIComponent(assertRegisteredName(name))}`
    );
  }

  set(
    name: string,
    value: string,
    options: RevisionMutationOptions = {}
  ): Promise<SecretMetadataV1> {
    return this.#http.request<SecretMetadataV1>(
      `/api/workspace/secrets/${encodeURIComponent(assertRegisteredName(name))}`,
      {
        method: "PUT",
        headers: mutationHeaders(options),
        body: JSON.stringify({ value })
      }
    );
  }

  async delete(name: string): Promise<void> {
    await this.#http.request<void>(
      `/api/workspace/secrets/${encodeURIComponent(assertRegisteredName(name))}`,
      { method: "DELETE" }
    );
  }

  revoke(
    name: string,
    options: IdempotencyOptions = {}
  ): Promise<SecretRevocation> {
    return this.#http.request<SecretRevocation>(
      `/api/workspace/secrets/${encodeURIComponent(assertRegisteredName(name))}/revocations`,
      {
        method: "POST",
        headers: idempotencyHeaders(options),
        body: JSON.stringify({})
      }
    );
  }
}

export class WorkspaceClient {
  readonly files: RegisteredResourceClient<"file">;
  readonly skills: RegisteredResourceClient<"skill">;
  readonly tools: RegisteredResourceClient<"tool">;
  readonly instructions: RegisteredResourceClient<"instruction">;
  readonly mcpServers: RegisteredResourceClient<"mcp_server">;
  readonly uploads: WorkspaceUploadsClient;
  readonly secrets: WorkspaceSecretsClient;

  constructor(http: HttpClient) {
    this.files = new RegisteredResourceClient(http, "file");
    this.skills = new RegisteredResourceClient(http, "skill");
    this.tools = new RegisteredResourceClient(http, "tool");
    this.instructions = new RegisteredResourceClient(http, "instruction");
    this.mcpServers = new RegisteredResourceClient(http, "mcp_server");
    this.uploads = new WorkspaceUploadsClient(http);
    this.secrets = new WorkspaceSecretsClient(http);
  }
}

export class Aex {
  readonly sessions: SessionsClient;
  readonly operations: OperationsClient;
  readonly workspace: WorkspaceClient;

  constructor(apiKey: string, options?: Omit<AexOptions, "apiKey">);
  constructor(options: AexOptions);
  constructor(
    options: string | AexOptions,
    overrides: Omit<AexOptions, "apiKey"> = {}
  ) {
    const resolved = typeof options === "string"
      ? { ...overrides, apiKey: options }
      : options;
    if (!resolved.apiKey) {
      throw new CredentialValidationError("Aex: apiKey is required");
    }
    const debug = resolved.debug === true
      ? (line: string) => console.error(line)
      : resolved.debug || undefined;
    const baseUrl = resolved.baseUrl ?? regionalBaseUrl(resolved.apiKey);
    const http = new HttpClient({
      apiKey: resolved.apiKey,
      ...(baseUrl ? { baseUrl } : {}),
      ...(resolved.fetch ? { fetch: resolved.fetch } : {}),
      ...(debug ? { debug } : {}),
      retry: resolved.retry === false
        ? false
        : resolved.retry ?? HTTP_RETRY_POLICY
    });
    this.sessions = new SessionsClient(http);
    this.operations = new OperationsClient(http);
    this.workspace = new WorkspaceClient(http);
  }
}

function getRun(http: HttpClient, sessionId: string, runId: string): Promise<RunV1> {
  const session = assertId("session", sessionId, "run.sessionId");
  const run = assertId("run", runId, "run.id");
  return http.request<RunV1>(
    `/api/sessions/${encodeURIComponent(session)}/runs/${encodeURIComponent(run)}`
  );
}

function getOperation(http: HttpClient, operationId: string): Promise<Operation> {
  const id = assertId("operation", operationId, "operationId");
  return http.request<Operation>(
    `/api/operations/${encodeURIComponent(id)}`
  );
}

function idempotencyHeaders(options: IdempotencyOptions): Record<string, string> {
  const key = options.idempotencyKey ?? `idem_${crypto.randomUUID()}`;
  if (typeof key !== "string" || key.trim().length === 0) {
    throw new Error("idempotencyKey must be a non-empty string");
  }
  return { "Idempotency-Key": key };
}

function mutationHeaders(options: RevisionMutationOptions): Record<string, string> {
  return {
    ...idempotencyHeaders(options),
    ...(options.ifRevision === undefined
      ? {}
      : { "If-Match": revisionEtag(options.ifRevision) })
  };
}

function revisionHeaders(options: RevisionOptions): Record<string, string> | undefined {
  return options.ifRevision === undefined
    ? undefined
    : { "If-Match": revisionEtag(options.ifRevision) };
}

function revisionEtag(revision: number): string {
  if (!Number.isSafeInteger(revision) || revision < 1) {
    throw new Error("ifRevision must be a positive safe integer");
  }
  return `"${revision}"`;
}

function assertRegisteredName(name: string): string {
  if (!RegisteredNameSchema.safeParse(name).success) {
    throw new Error(
      "name must match [A-Za-z0-9][A-Za-z0-9._-]{0,127}"
    );
  }
  return name;
}

function assertGeneration(generationId: string | undefined): void {
  if (generationId !== undefined) {
    assertId("generation", generationId, "ifGenerationId");
  }
}

function assertByteRange(
  range: { readonly start: number; readonly endExclusive: number } | undefined
): void {
  if (range === undefined) return;
  if (
    !Number.isSafeInteger(range.start) ||
    range.start < 0 ||
    !Number.isSafeInteger(range.endExclusive) ||
    range.endExclusive <= range.start
  ) {
    throw new Error(
      "range must be a non-negative half-open byte interval with endExclusive > start"
    );
  }
}

function queryParameters<T extends object>(query: T): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) result[key] = String(value);
  }
  return result;
}

async function pollUntilTerminal<T>(
  initial: T,
  terminal: (value: T) => boolean,
  read: () => Promise<T>,
  options: WaitOptions
): Promise<T> {
  const interval = options.pollIntervalMs ?? 1_000;
  if (!Number.isFinite(interval) || interval < 0) {
    throw new Error("pollIntervalMs must be a finite non-negative number");
  }
  const startedAt = Date.now();
  let current = initial;
  while (!terminal(current)) {
    if (options.signal?.aborted) {
      throw options.signal.reason ?? new Error("Polling aborted");
    }
    if (
      options.timeoutMs !== undefined &&
      Date.now() - startedAt >= options.timeoutMs
    ) {
      throw new Error(`Polling timed out after ${options.timeoutMs}ms`);
    }
    if (interval > 0) {
      await delay(interval, options.signal);
    }
    current = await read();
  }
  return current;
}

async function delay(ms: number, signal?: AbortSignal): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal?.reason ?? new Error("Polling aborted"));
      return;
    }
    const finish = () => {
      if (signal !== undefined) signal.removeEventListener("abort", abort);
      resolve();
    };
    const timer = setTimeout(finish, ms);
    const abort = () => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", abort);
      reject(signal?.reason ?? new Error("Polling aborted"));
    };
    signal?.addEventListener("abort", abort, { once: true });
  });
}

function regionalBaseUrl(apiKey: string): string | undefined {
  const parsed = parseApiKey(apiKey);
  return parsed === null ? undefined : `https://${parsed.region}.api.aex.dev`;
}
