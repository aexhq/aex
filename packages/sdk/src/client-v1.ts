import {
  AEX_DEFAULT_BASE_URL,
  CredentialValidationError,
  HTTP_RETRY_POLICY,
  HttpClient,
  assertId,
  apiErrorFromResponse,
  AutoTopupPolicyRequestSchema,
  newId,
  ObservationFrameSchema,
  parseApiKey,
  RegisteredNameSchema,
  type Approval,
  type ApprovalResponseRequest,
  type AccountOperationalState,
  type ApiKey,
  type ApiKeyCreateRequest,
  type AutoTopupPolicy,
  type AutoTopupPolicyRequest,
  type BillingBalance,
  type DebugSink,
  type DownloadGrant,
  type EffectiveWorkspaceLimit,
  type FileDownloadRequest,
  type FileEntry,
  type FetchLike,
  type HttpRetryOptions,
  type Id,
  type HostedBillingSession,
  type LiveDownloadGrant,
  type LiveFileDownloadRequest,
  type LiveFileEntry,
  type LiveFileListRequest,
  type LiveFilePage,
  type LiveFileStatRequest,
  type MessageSendRequest,
  type MessageV1,
  type Membership,
  type MetricAggregationPage,
  type MetricAggregationRequest,
  type Observation,
  type ObservationCoverage,
  type ObservationFrame,
  type ObservationListenRequest,
  type ObservationPage,
  type ObservationQuery,
  type ObservationSignal,
  type ObservationStreamRequest,
  type Organization,
  type OrganizationCreateRequest,
  type OrganizationUsageQuery,
  type OrganizationUsageResult,
  type Operation,
  type OperationKind,
  type OperationStatusV1,
  type Page,
  type PersistedFileListRequest,
  type PersistedFileStatRequest,
  type PortalSessionRequest,
  type RegisteredFileDownloadRequest,
  type RegisteredResource,
  type RegisteredResourceInput,
  type RegisteredResourceKind,
  type RegisteredResourceSummary,
  type RunStatusV1,
  type RunV1,
  type SecretMetadataV1,
  type SecretRevocation,
  type SessionCreateRequestV1,
  type SessionStatusV1,
  type SessionV1,
  type TelemetryExport,
  type TelemetryExportRequest,
  type TelemetryGap,
  type TelemetryGapQuery,
  type Statement,
  type StatementSummary,
  type TopUpCheckoutRequest,
  type TraceSummary,
  type UsageFrontier,
  type UsagePage,
  type UsageQuery,
  type Upload,
  type UploadCompleteRequest,
  type UploadCreateRequest,
  type UploadPartsRequest,
  type UploadPartsResponse,
  type Invitation,
  type InvitationCreateRequest,
  type Workspace,
  type WorkspaceCreateRequest,
  type NewWorkspaceApiKey
} from "@aexhq/contracts";

export interface AexOptions {
  readonly apiKey?: string;
  readonly bootstrapBaseUrl?: string;
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

export interface AccountGetQuery {
  readonly organizationId?: string;
}

export interface OrganizationListQuery {
  readonly cursor?: string;
  readonly limit?: number;
}

export interface WorkspaceListQuery {
  readonly organizationId?: string;
  readonly cursor?: string;
  readonly limit?: number;
}

export interface ApiKeyListQuery {
  readonly workspaceId: string;
  readonly cursor?: string;
  readonly limit?: number;
}

export interface BillingBalanceQuery {
  readonly organizationId?: string;
}

export interface AutoTopupReplaceOptions extends IdempotencyOptions {
  readonly ifRevision: number;
}

export interface WorkspaceDeleteOptions extends OperationAdmissionOptions {
  readonly confirmation: string;
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

export class TelemetryStreamProtocolError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "TelemetryStreamProtocolError";
  }
}

export class TelemetryStreamBackpressureError extends Error {
  readonly cursor: string | undefined;

  constructor(message: string, cursor?: string) {
    super(message);
    this.name = "TelemetryStreamBackpressureError";
    this.cursor = cursor;
  }
}

export class TelemetryStreamRotationError extends Error {
  readonly cursor: string | undefined;

  constructor(message: string, cursor?: string) {
    super(message);
    this.name = "TelemetryStreamRotationError";
    this.cursor = cursor;
  }
}

class RawTelemetryTransport {
  readonly #baseUrl: URL;
  readonly #apiKey: string;
  readonly #fetch: FetchLike;

  constructor(baseUrl: string, apiKey: string, fetchImpl: FetchLike) {
    this.#baseUrl = new URL(baseUrl.endsWith("/") ? baseUrl : `${baseUrl}/`);
    this.#apiKey = apiKey;
    this.#fetch = fetchImpl;
  }

  async post(
    path: string,
    body: BodyInit,
    headers: HeadersInit
  ): Promise<Response> {
    const response = await this.#fetch(
      new URL(path.replace(/^\//, ""), this.#baseUrl),
      {
        method: "POST",
        headers: {
          accept: "application/json",
          authorization: `Bearer ${this.#apiKey}`,
          ...Object.fromEntries(new Headers(headers))
        },
        body
      }
    );
    if (!response.ok) {
      let parsed: unknown;
      try {
        parsed = await response.clone().json();
      } catch {
        parsed = undefined;
      }
      throw apiErrorFromResponse({
        status: response.status,
        body: parsed,
        message: telemetryErrorMessage(parsed, response.status)
      });
    }
    return response;
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

type ObservationFor<S extends ObservationSignal | "telemetry"> =
  S extends "telemetry" ? Observation : Extract<Observation, { readonly signal: S }>;

export class ObservationIterator<S extends ObservationSignal | "telemetry">
implements AsyncIterable<ObservationFor<S>> {
  readonly #client: ObservationSignalClient<S>;
  readonly #query: ObservationQuery;
  readonly coverage: Promise<ObservationCoverage>;
  #resolveCoverage!: (coverage: ObservationCoverage) => void;
  #rejectCoverage!: (error: unknown) => void;
  #started = false;

  constructor(client: ObservationSignalClient<S>, query: ObservationQuery) {
    this.#client = client;
    this.#query = query;
    this.coverage = new Promise<ObservationCoverage>((resolve, reject) => {
      this.#resolveCoverage = resolve;
      this.#rejectCoverage = reject;
    });
  }

  [Symbol.asyncIterator](): AsyncIterator<ObservationFor<S>> {
    if (this.#started) {
      throw new TelemetryStreamProtocolError(
        "ObservationIterator can be consumed only once"
      );
    }
    this.#started = true;
    return this.#iterate();
  }

  async *#iterate(): AsyncGenerator<ObservationFor<S>, void, void> {
    let cursor = this.#query.cursor;
    let finalCoverage: ObservationCoverage | undefined;
    const seen = new Set<string>();
    try {
      do {
        if (cursor !== undefined && seen.has(cursor)) {
          throw new TelemetryStreamProtocolError(
            `Observation query repeated cursor ${cursor}`
          );
        }
        if (cursor !== undefined) seen.add(cursor);
        const page = await this.#client.query({
          ...this.#query,
          ...(cursor === undefined ? {} : { cursor })
        });
        finalCoverage = page.coverage;
        for (const item of page.items) yield item;
        cursor = page.nextCursor;
      } while (cursor !== undefined);
      if (finalCoverage === undefined) {
        throw new TelemetryStreamProtocolError(
          "Observation iteration completed without coverage"
        );
      }
      this.#resolveCoverage(finalCoverage);
    } catch (error) {
      this.#rejectCoverage(error);
      throw error;
    }
  }
}

export class ObservationSignalClient<S extends ObservationSignal | "telemetry"> {
  readonly #http: HttpClient;
  readonly #raw: RawTelemetryTransport;
  readonly #path: string;

  constructor(
    http: HttpClient,
    raw: RawTelemetryTransport,
    signal: S,
    sessionId?: string
  ) {
    this.#http = http;
    this.#raw = raw;
    this.#path = sessionId === undefined
      ? `/api/${signal}`
      : `/api/sessions/${encodeURIComponent(sessionId)}/${signal}`;
  }

  query(request: ObservationQuery = {}): Promise<
    Omit<ObservationPage, "items"> & { readonly items: readonly ObservationFor<S>[] }
  > {
    return this.#http.request<
      Omit<ObservationPage, "items"> & { readonly items: readonly ObservationFor<S>[] }
    >(`${this.#path}/query`, {
      method: "POST",
      body: JSON.stringify(request)
    });
  }

  iterate(request: ObservationQuery = {}): ObservationIterator<S> {
    return new ObservationIterator(this, request);
  }

  stream(request: ObservationStreamRequest): AsyncIterable<ObservationFrame> {
    return this.#frames("stream", request);
  }

  listen(request: ObservationListenRequest = {}): AsyncIterable<ObservationFrame> {
    return this.#frames("listen", request);
  }

  async *#frames(
    action: "stream" | "listen",
    request: ObservationStreamRequest | ObservationListenRequest
  ): AsyncGenerator<ObservationFrame, void, void> {
    const response = await this.#raw.post(
      `${this.#path}/${action}`,
      JSON.stringify(request),
      {
        accept: "application/x-ndjson",
        "content-type": "application/json"
      }
    );
    const closeReason = response.headers.get("aex-stream-close");
    if (closeReason === "backpressure") {
      throw new TelemetryStreamBackpressureError(
        "Telemetry stream closed for slow-consumer backpressure",
        response.headers.get("aex-stream-cursor") ?? undefined
      );
    }
    if (closeReason === "rotation") {
      throw new TelemetryStreamRotationError(
        "Telemetry stream rotated without a rotate frame",
        response.headers.get("aex-stream-cursor") ?? undefined
      );
    }
    for await (const line of ndjsonLines(response)) {
      let value: unknown;
      try {
        value = JSON.parse(line);
      } catch (cause) {
        throw new TelemetryStreamProtocolError(
          "Telemetry stream returned malformed NDJSON",
          { cause }
        );
      }
      const parsed = ObservationFrameSchema.safeParse(value);
      if (!parsed.success) {
        throw new TelemetryStreamProtocolError(
          "Telemetry stream returned an invalid frame"
        );
      }
      yield parsed.data;
    }
  }
}

export class MetricsClient extends ObservationSignalClient<"metrics"> {
  readonly #http: HttpClient;
  readonly #path: string;

  constructor(http: HttpClient, raw: RawTelemetryTransport, sessionId?: string) {
    super(http, raw, "metrics", sessionId);
    this.#http = http;
    this.#path = sessionId === undefined
      ? "/api/metrics"
      : `/api/sessions/${encodeURIComponent(sessionId)}/metrics`;
  }

  aggregate(request: MetricAggregationRequest): Promise<MetricAggregationPage> {
    return this.#http.request<MetricAggregationPage>(
      `${this.#path}/aggregate`,
      { method: "POST", body: JSON.stringify(request) }
    );
  }
}

export class TracesClient extends ObservationSignalClient<"traces"> {
  readonly #http: HttpClient;
  readonly #sessionId: string | undefined;

  constructor(http: HttpClient, raw: RawTelemetryTransport, sessionId?: string) {
    super(http, raw, "traces", sessionId);
    this.#http = http;
    this.#sessionId = sessionId;
  }

  get(traceId: string): Promise<TraceSummary> {
    if (this.#sessionId === undefined) {
      throw new Error("traces.get requires a session-scoped trace client");
    }
    if (!/^[0-9a-f]{32}$/.test(traceId)) {
      throw new Error("traceId must be a 32-character lowercase W3C trace id");
    }
    return this.#http.request<TraceSummary>(
      `/api/sessions/${encodeURIComponent(this.#sessionId)}/traces/${traceId}`
    );
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
  readonly events: ObservationSignalClient<"events">;
  readonly logs: ObservationSignalClient<"logs">;
  readonly spans: ObservationSignalClient<"spans">;
  readonly metrics: MetricsClient;
  readonly traces: TracesClient;
  readonly telemetry: ScopedTelemetryClient;

  constructor(
    http: HttpClient,
    raw: RawTelemetryTransport,
    session: SessionV1
  ) {
    this.#http = http;
    assertId("session", session.id, "session.id");
    this.#session = session;
    this.messages = new SessionMessagesClient(http, session.id);
    this.workspace = new SessionWorkspaceClient(this);
    this.credentials = new SessionCredentialsClient(this);
    this.files = new SessionFilesClient(http, session.id);
    this.approvals = new SessionApprovalsClient(http, session.id);
    this.events = new ObservationSignalClient(http, raw, "events", session.id);
    this.logs = new ObservationSignalClient(http, raw, "logs", session.id);
    this.spans = new ObservationSignalClient(http, raw, "spans", session.id);
    this.metrics = new MetricsClient(http, raw, session.id);
    this.traces = new TracesClient(http, raw, session.id);
    this.telemetry = new ScopedTelemetryClient(http, raw, session.id);
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
  readonly #raw: RawTelemetryTransport;

  constructor(http: HttpClient, raw: RawTelemetryTransport) {
    this.#http = http;
    this.#raw = raw;
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
    return new SessionHandle(this.#http, this.#raw, session);
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
    return new SessionHandle(this.#http, this.#raw, await this.get(sessionId));
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

export class AccountClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  get(query: AccountGetQuery = {}): Promise<AccountOperationalState> {
    if (query.organizationId !== undefined) {
      assertId("organization", query.organizationId, "organizationId");
    }
    return this.#http.request<AccountOperationalState>(
      "/api/account",
      {},
      queryParameters(query)
    );
  }
}

export class OrganizationMembershipsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(
    organizationId: string,
    query: OrganizationListQuery = {}
  ): Promise<Page<Membership>> {
    const id = assertId("organization", organizationId, "organizationId");
    return this.#http.request<Page<Membership>>(
      `/api/organizations/${encodeURIComponent(id)}/memberships`,
      {},
      queryParameters(query)
    );
  }
}

export class OrganizationInvitationsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  create(
    organizationId: string,
    request: InvitationCreateRequest,
    options: IdempotencyOptions = {}
  ): Promise<Invitation> {
    const id = assertId("organization", organizationId, "organizationId");
    return this.#http.request<Invitation>(
      `/api/organizations/${encodeURIComponent(id)}/invitations`,
      {
        method: "POST",
        headers: idempotencyHeaders(options),
        body: JSON.stringify(request)
      }
    );
  }
}

export class OrganizationsClient {
  readonly #http: HttpClient;
  readonly memberships: OrganizationMembershipsClient;
  readonly invitations: OrganizationInvitationsClient;

  constructor(http: HttpClient) {
    this.#http = http;
    this.memberships = new OrganizationMembershipsClient(http);
    this.invitations = new OrganizationInvitationsClient(http);
  }

  list(query: OrganizationListQuery = {}): Promise<Page<Organization>> {
    return this.#http.request<Page<Organization>>(
      "/api/organizations",
      {},
      queryParameters(query)
    );
  }

  create(
    request: OrganizationCreateRequest,
    options: IdempotencyOptions = {}
  ): Promise<Organization> {
    return this.#http.request<Organization>("/api/organizations", {
      method: "POST",
      headers: idempotencyHeaders(options),
      body: JSON.stringify(request)
    });
  }

  async get(organizationId: string): Promise<Organization> {
    const id = assertId("organization", organizationId, "organizationId");
    const organization = await this.#http.request<Organization>(
      `/api/organizations/${encodeURIComponent(id)}`
    );
    if (organization.id !== id) {
      throw new Error(`Organization GET for ${id} returned ${organization.id}`);
    }
    return organization;
  }
}

export class WorkspacesClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(query: WorkspaceListQuery = {}): Promise<Page<Workspace>> {
    if (query.organizationId !== undefined) {
      assertId("organization", query.organizationId, "organizationId");
    }
    return this.#http.request<Page<Workspace>>(
      "/api/workspaces",
      {},
      queryParameters(query)
    );
  }

  create(
    request: WorkspaceCreateRequest,
    options: IdempotencyOptions = {}
  ): Promise<Workspace> {
    assertId("organization", request.organizationId, "request.organizationId");
    return this.#http.request<Workspace>("/api/workspaces", {
      method: "POST",
      headers: idempotencyHeaders(options),
      body: JSON.stringify(request)
    });
  }

  async get(workspaceId: string): Promise<Workspace> {
    const id = assertId("workspace", workspaceId, "workspaceId");
    const workspace = await this.#http.request<Workspace>(
      `/api/workspaces/${encodeURIComponent(id)}`
    );
    if (workspace.id !== id) {
      throw new Error(`Workspace GET for ${id} returned ${workspace.id}`);
    }
    return workspace;
  }

  async delete(
    workspaceId: string,
    options: WorkspaceDeleteOptions
  ): Promise<OperationHandle<"workspace_delete">> {
    const id = assertId("workspace", workspaceId, "workspaceId");
    const confirmation = assertId(
      "workspace",
      options.confirmation,
      "confirmation"
    );
    if (confirmation !== id) {
      throw new Error("confirmation must exactly match workspaceId");
    }
    const operationId = options.operationId === undefined
      ? newId("operation")
      : assertId("operation", options.operationId, "operationId");
    const operation = await this.#http.request<OperationFor<"workspace_delete">>(
      `/api/workspaces/${encodeURIComponent(id)}/deletions`,
      {
        method: "POST",
        headers: { "Aex-Operation-Id": operationId },
        body: JSON.stringify({ confirmation })
      }
    );
    if (operation.id !== operationId || operation.kind !== "workspace_delete") {
      throw new Error(
        `Workspace deletion for ${operationId} returned ${operation.id}/${operation.kind}`
      );
    }
    return new OperationHandle(this.#http, operation);
  }
}

export class ApiKeysClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(query: ApiKeyListQuery): Promise<Page<ApiKey>> {
    assertId("workspace", query.workspaceId, "workspaceId");
    return this.#http.request<Page<ApiKey>>(
      "/api/api-keys",
      {},
      queryParameters(query)
    );
  }

  create(
    request: ApiKeyCreateRequest,
    options: IdempotencyOptions = {}
  ): Promise<NewWorkspaceApiKey> {
    assertId("workspace", request.workspaceId, "request.workspaceId");
    return this.#http.request<NewWorkspaceApiKey>("/api/api-keys", {
      method: "POST",
      headers: idempotencyHeaders(options),
      body: JSON.stringify(request)
    });
  }

  async revoke(apiKeyId: string, options: RevisionOptions = {}): Promise<void> {
    const id = assertId("apiKey", apiKeyId, "apiKeyId");
    const headers = revisionHeaders(options);
    await this.#http.request<void>(
      `/api/api-keys/${encodeURIComponent(id)}`,
      {
        method: "DELETE",
        ...(headers ? { headers } : {})
      }
    );
  }
}

export class BillingBalanceClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  get(query: BillingBalanceQuery = {}): Promise<BillingBalance> {
    if (query.organizationId !== undefined) {
      assertId("organization", query.organizationId, "organizationId");
    }
    return this.#http.request<BillingBalance>(
      "/api/billing/balance",
      {},
      queryParameters(query)
    );
  }
}

export class BillingAutoTopupClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  get(organizationId: string): Promise<AutoTopupPolicy> {
    const id = assertId("organization", organizationId, "organizationId");
    return this.#http.request<AutoTopupPolicy>(
      `/api/organizations/${encodeURIComponent(id)}/billing/auto-topup-policy`
    );
  }

  replace(
    organizationId: string,
    request: AutoTopupPolicyRequest,
    options: AutoTopupReplaceOptions
  ): Promise<AutoTopupPolicy> {
    const id = assertId("organization", organizationId, "organizationId");
    if (!AutoTopupPolicyRequestSchema.safeParse(request).success) {
      throw new Error(
        "auto-topup policy must be complete and use valid whole-cent USD bounds"
      );
    }
    return this.#http.request<AutoTopupPolicy>(
      `/api/organizations/${encodeURIComponent(id)}/billing/auto-topup-policy`,
      {
        method: "PUT",
        headers: {
          ...idempotencyHeaders(options),
          "If-Match": revisionEtag(options.ifRevision)
        },
        body: JSON.stringify(request)
      }
    );
  }
}

export class BillingStatementsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(
    organizationId: string,
    query: OrganizationListQuery = {}
  ): Promise<Page<StatementSummary>> {
    const id = assertId("organization", organizationId, "organizationId");
    return this.#http.request<Page<StatementSummary>>(
      `/api/organizations/${encodeURIComponent(id)}/billing/statements`,
      {},
      queryParameters(query)
    );
  }

  async get(organizationId: string, statementId: string): Promise<Statement> {
    const organization = assertId(
      "organization",
      organizationId,
      "organizationId"
    );
    const statement = assertId("statement", statementId, "statementId");
    const record = await this.#http.request<Statement>(
      `/api/organizations/${encodeURIComponent(organization)}/billing/statements/${encodeURIComponent(statement)}`
    );
    if (record.id !== statement || record.organizationId !== organization) {
      throw new Error(
        `Statement GET for ${organization}/${statement} returned ${record.organizationId}/${record.id}`
      );
    }
    return record;
  }

  download(
    organizationId: string,
    statementId: string,
    options: IdempotencyOptions = {}
  ): Promise<DownloadGrant> {
    const organization = assertId(
      "organization",
      organizationId,
      "organizationId"
    );
    const statement = assertId("statement", statementId, "statementId");
    return this.#http.request<DownloadGrant>(
      `/api/organizations/${encodeURIComponent(organization)}/billing/statements/${encodeURIComponent(statement)}/downloads`,
      {
        method: "POST",
        headers: idempotencyHeaders(options),
        body: JSON.stringify({})
      }
    );
  }
}

type RegionalHttpFactory = (baseUrl: string) => HttpClient;

export class BillingUsageClient {
  readonly #regionalHttp: HttpClient;
  readonly #workspaces: WorkspacesClient;
  readonly #regionalHttpFor: RegionalHttpFactory;

  constructor(
    regionalHttp: HttpClient,
    workspaces: WorkspacesClient,
    regionalHttpFor: RegionalHttpFactory
  ) {
    this.#regionalHttp = regionalHttp;
    this.#workspaces = workspaces;
    this.#regionalHttpFor = regionalHttpFor;
  }

  query(request: UsageQuery): Promise<UsagePage> {
    return this.#query(this.#regionalHttp, request);
  }

  async queryOrganization(
    organizationId: string,
    request: OrganizationUsageQuery
  ): Promise<OrganizationUsageResult> {
    const organization = assertId(
      "organization",
      organizationId,
      "organizationId"
    );
    const workspaces = await this.#allWorkspaces(organization);
    const results = await Promise.all(
      workspaces.map(async (workspace) => {
        const http = this.#regionalHttpFor(workspace.apiUrl);
        const items = [];
        const frontiers: UsageFrontier[] = [];
        let cursor: string | undefined;
        const seen = new Set<string>();
        do {
          if (cursor !== undefined && seen.has(cursor)) {
            throw new Error(
              `Usage query repeated cursor ${cursor} for workspace ${workspace.id}`
            );
          }
          if (cursor !== undefined) seen.add(cursor);
          const page = await this.#query(
            http,
            { ...request, ...(cursor === undefined ? {} : { cursor }) },
            workspace.id
          );
          items.push(...page.items);
          frontiers.push(...page.frontiers);
          cursor = page.nextCursor;
        } while (cursor !== undefined);
        return { items, frontiers };
      })
    );
    return {
      items: results.flatMap(({ items }) => items),
      frontiers: mergeUsageFrontiers(
        results.flatMap(({ frontiers }) => frontiers)
      )
    };
  }

  #query(
    http: HttpClient,
    request: UsageQuery,
    workspaceId?: string
  ): Promise<UsagePage> {
    return http.request<UsagePage>(
      "/api/billing/usage/query",
      { method: "POST", body: JSON.stringify(request) },
      workspaceId === undefined ? undefined : { workspaceId }
    );
  }

  async #allWorkspaces(organizationId: string): Promise<readonly Workspace[]> {
    const items: Workspace[] = [];
    let cursor: string | undefined;
    const seen = new Set<string>();
    do {
      if (cursor !== undefined && seen.has(cursor)) {
        throw new Error(`Workspace list repeated cursor ${cursor}`);
      }
      if (cursor !== undefined) seen.add(cursor);
      const page = await this.#workspaces.list({
        organizationId,
        ...(cursor === undefined ? {} : { cursor }),
        limit: 1_000
      });
      items.push(...page.items);
      cursor = page.nextCursor;
    } while (cursor !== undefined);
    return items;
  }
}

export class BillingClient {
  readonly #http: HttpClient;
  readonly balance: BillingBalanceClient;
  readonly autoTopup: BillingAutoTopupClient;
  readonly statements: BillingStatementsClient;
  readonly usage: BillingUsageClient;

  constructor(
    bootstrapHttp: HttpClient,
    regionalHttp: HttpClient,
    workspaces: WorkspacesClient,
    regionalHttpFor: RegionalHttpFactory
  ) {
    this.#http = bootstrapHttp;
    this.balance = new BillingBalanceClient(bootstrapHttp);
    this.autoTopup = new BillingAutoTopupClient(bootstrapHttp);
    this.statements = new BillingStatementsClient(bootstrapHttp);
    this.usage = new BillingUsageClient(
      regionalHttp,
      workspaces,
      regionalHttpFor
    );
  }

  topUpCheckout(
    organizationId: string,
    request: TopUpCheckoutRequest,
    options: IdempotencyOptions = {}
  ): Promise<HostedBillingSession> {
    return this.#hosted(
      organizationId,
      "top-up-checkouts",
      request,
      options
    );
  }

  portalSession(
    organizationId: string,
    request: PortalSessionRequest = {},
    options: IdempotencyOptions = {}
  ): Promise<HostedBillingSession> {
    return this.#hosted(
      organizationId,
      "portal-sessions",
      request,
      options
    );
  }

  #hosted(
    organizationId: string,
    route: "top-up-checkouts" | "portal-sessions",
    request: TopUpCheckoutRequest | PortalSessionRequest,
    options: IdempotencyOptions
  ): Promise<HostedBillingSession> {
    const id = assertId("organization", organizationId, "organizationId");
    return this.#http.request<HostedBillingSession>(
      `/api/organizations/${encodeURIComponent(id)}/billing/${route}`,
      {
        method: "POST",
        headers: idempotencyHeaders(options),
        body: JSON.stringify(request)
      }
    );
  }
}

type RegisteredResourceFor<K extends RegisteredResourceKind> =
  Extract<RegisteredResource, { readonly kind: K }>;
type RegisteredSummaryFor<K extends RegisteredResourceKind> =
  Extract<RegisteredResourceSummary, { readonly kind: K }>;
export type RegistryPutResultFor<K extends RegisteredResourceKind> = {
  readonly status: "created" | "replaced" | "unchanged";
  readonly resource: RegisteredResourceFor<K>;
};

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
    value: RegisteredResourceInput<K>,
    options: RevisionMutationOptions = {}
  ): Promise<RegistryPutResultFor<K>> {
    const result = await this.#http.request<RegistryPutResultFor<K>>(
      `/api/workspace/${this.#path}/${encodeURIComponent(assertRegisteredName(name))}`,
      {
        method: "PUT",
        headers: mutationHeaders(options),
        body: JSON.stringify(value)
      }
    );
    return {
      ...result,
      resource: this.#check(result.resource, name)
    };
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

export class RegisteredFileClient extends RegisteredResourceClient<"file"> {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    super(http, "file");
    this.#http = http;
  }

  download(
    name: string,
    request: RegisteredFileDownloadRequest = {},
    options: IdempotencyOptions = {}
  ): Promise<DownloadGrant> {
    return this.#http.request<DownloadGrant>(
      `/api/workspace/files/${encodeURIComponent(assertRegisteredName(name))}/downloads`,
      {
        method: "POST",
        headers: idempotencyHeaders(options),
        body: JSON.stringify(request)
      }
    );
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
  readonly #http: HttpClient;
  readonly files: RegisteredFileClient;
  readonly skills: RegisteredResourceClient<"skill">;
  readonly tools: RegisteredResourceClient<"tool">;
  readonly instructions: RegisteredResourceClient<"instruction">;
  readonly mcpServers: RegisteredResourceClient<"mcp_server">;
  readonly uploads: WorkspaceUploadsClient;
  readonly secrets: WorkspaceSecretsClient;
  readonly limits: WorkspaceLimitsClient;

  constructor(http: HttpClient) {
    this.#http = http;
    this.files = new RegisteredFileClient(http);
    this.skills = new RegisteredResourceClient(http, "skill");
    this.tools = new RegisteredResourceClient(http, "tool");
    this.instructions = new RegisteredResourceClient(http, "instruction");
    this.mcpServers = new RegisteredResourceClient(http, "mcp_server");
    this.uploads = new WorkspaceUploadsClient(http);
    this.secrets = new WorkspaceSecretsClient(http);
    this.limits = new WorkspaceLimitsClient(http);
  }

  get(): Promise<Workspace> {
    return this.#http.request<Workspace>("/api/workspace");
  }
}

export class WorkspaceLimitsClient {
  readonly #http: HttpClient;

  constructor(http: HttpClient) {
    this.#http = http;
  }

  list(query: { readonly cursor?: string; readonly limit?: number } = {}):
    Promise<Page<EffectiveWorkspaceLimit>> {
    return this.#http.request<Page<EffectiveWorkspaceLimit>>(
      "/api/workspace/limits",
      {},
      queryParameters(query)
    );
  }

  async get(limitId: string): Promise<EffectiveWorkspaceLimit> {
    const id = assertLimitId(limitId);
    const limit = await this.#http.request<EffectiveWorkspaceLimit>(
      `/api/workspace/limits/${encodeURIComponent(id)}`
    );
    if (limit.id !== id) {
      throw new Error(`Workspace limit GET for ${id} returned ${limit.id}`);
    }
    return limit;
  }
}

export interface TelemetryGapPage {
  readonly items: readonly TelemetryGap[];
  readonly nextCursor?: string;
  readonly coverage: ObservationCoverage;
}

export class TelemetryGapsClient {
  readonly #http: HttpClient;
  readonly #prefix: string;

  constructor(http: HttpClient, sessionId?: string) {
    this.#http = http;
    this.#prefix = sessionId === undefined
      ? "/api/telemetry/gaps"
      : `/api/sessions/${encodeURIComponent(sessionId)}/telemetry/gaps`;
  }

  query(request: TelemetryGapQuery = {}): Promise<TelemetryGapPage> {
    return this.#http.request<TelemetryGapPage>(`${this.#prefix}/query`, {
      method: "POST",
      body: JSON.stringify(request)
    });
  }

  async get(gapId: string): Promise<TelemetryGap> {
    const id = assertId("telemetryGap", gapId, "gapId");
    const gap = await this.#http.request<TelemetryGap>(
      `${this.#prefix}/${encodeURIComponent(id)}`
    );
    if (gap.id !== id) {
      throw new Error(`Telemetry gap GET for ${id} returned ${gap.id}`);
    }
    return gap;
  }
}

export class TelemetryExportsClient {
  readonly #http: HttpClient;
  readonly #prefix: string;

  constructor(http: HttpClient, sessionId?: string) {
    this.#http = http;
    this.#prefix = sessionId === undefined
      ? "/api/telemetry/exports"
      : `/api/sessions/${encodeURIComponent(sessionId)}/telemetry/exports`;
  }

  async get(exportId: string): Promise<TelemetryExport> {
    const id = assertId("export", exportId, "exportId");
    const record = await this.#http.request<TelemetryExport>(
      `${this.#prefix}/${encodeURIComponent(id)}`
    );
    return this.#check(record, id);
  }

  download(
    exportId: string,
    options: IdempotencyOptions = {}
  ): Promise<DownloadGrant> {
    const id = assertId("export", exportId, "exportId");
    return this.#http.request<DownloadGrant>(
      `${this.#prefix}/${encodeURIComponent(id)}/downloads`,
      {
        method: "POST",
        headers: idempotencyHeaders(options),
        body: JSON.stringify({})
      }
    );
  }

  async revoke(
    exportId: string,
    options: IdempotencyOptions = {}
  ): Promise<TelemetryExport> {
    const id = assertId("export", exportId, "exportId");
    const record = await this.#http.request<TelemetryExport>(
      `${this.#prefix}/${encodeURIComponent(id)}/revocations`,
      {
        method: "POST",
        headers: idempotencyHeaders(options),
        body: JSON.stringify({})
      }
    );
    return this.#check(record, id);
  }

  #check(record: TelemetryExport, id: string): TelemetryExport {
    if (record.id !== id) {
      throw new Error(`Telemetry export ${id} returned ${record.id}`);
    }
    return record;
  }
}

export interface OtlpAdmissionOptions {
  readonly batchId?: Id<"telemetryBatch">;
  readonly contentType?: "application/json" | "application/x-protobuf";
}

export interface OtlpAdmissionReceipt {
  readonly batchId: string;
  readonly receiptId: string;
  readonly response: unknown;
}

export class TelemetryOtlpClient {
  readonly #raw: RawTelemetryTransport;

  constructor(raw: RawTelemetryTransport) {
    this.#raw = raw;
  }

  logs(body: unknown, options: OtlpAdmissionOptions = {}): Promise<OtlpAdmissionReceipt> {
    return this.#admit("logs", body, options);
  }

  traces(body: unknown, options: OtlpAdmissionOptions = {}): Promise<OtlpAdmissionReceipt> {
    return this.#admit("traces", body, options);
  }

  metrics(body: unknown, options: OtlpAdmissionOptions = {}): Promise<OtlpAdmissionReceipt> {
    return this.#admit("metrics", body, options);
  }

  async #admit(
    signal: "logs" | "traces" | "metrics",
    body: unknown,
    options: OtlpAdmissionOptions
  ): Promise<OtlpAdmissionReceipt> {
    const batchId = options.batchId === undefined
      ? newId("telemetryBatch")
      : assertId("telemetryBatch", options.batchId, "batchId");
    const contentType = options.contentType ?? "application/json";
    const wireBody: BodyInit = contentType === "application/json"
      ? JSON.stringify(body)
      : body as BodyInit;
    const response = await this.#raw.post(
      `/api/telemetry/otlp/v1/${signal}`,
      wireBody,
      {
        accept: "application/json",
        "content-type": contentType,
        "Idempotency-Key": batchId
      }
    );
    const returnedBatchId = response.headers.get("aex-telemetry-batch-id");
    const receiptId = response.headers.get("aex-telemetry-receipt-id");
    if (returnedBatchId !== batchId || receiptId === null) {
      throw new TelemetryStreamProtocolError(
        "OTLP admission response is missing its batch or receipt identity"
      );
    }
    let parsed: unknown;
    try {
      parsed = await response.json();
    } catch {
      parsed = {};
    }
    return { batchId, receiptId, response: parsed };
  }
}

export class ScopedTelemetryClient extends ObservationSignalClient<"telemetry"> {
  readonly #http: HttpClient;
  readonly #prefix: string;
  readonly gaps: TelemetryGapsClient;
  readonly exports: TelemetryExportsClient;

  constructor(http: HttpClient, raw: RawTelemetryTransport, sessionId?: string) {
    super(http, raw, "telemetry", sessionId);
    this.#http = http;
    this.#prefix = sessionId === undefined
      ? "/api/telemetry"
      : `/api/sessions/${encodeURIComponent(sessionId)}/telemetry`;
    this.gaps = new TelemetryGapsClient(http, sessionId);
    this.exports = new TelemetryExportsClient(http, sessionId);
  }

  async export(
    request: TelemetryExportRequest,
    options: OperationAdmissionOptions = {}
  ): Promise<OperationHandle<"telemetry_export">> {
    const operationId = options.operationId === undefined
      ? newId("operation")
      : assertId("operation", options.operationId, "operationId");
    const operation = await this.#http.request<OperationFor<"telemetry_export">>(
      `${this.#prefix}/exports`,
      {
        method: "POST",
        headers: { "Aex-Operation-Id": operationId },
        body: JSON.stringify(request)
      }
    );
    if (operation.id !== operationId || operation.kind !== "telemetry_export") {
      throw new Error(
        `Telemetry export admission for ${operationId} returned ${operation.id}/${operation.kind}`
      );
    }
    return new OperationHandle(this.#http, operation);
  }
}

export class TelemetryClient extends ScopedTelemetryClient {
  readonly otlp: TelemetryOtlpClient;

  constructor(http: HttpClient, raw: RawTelemetryTransport) {
    super(http, raw);
    this.otlp = new TelemetryOtlpClient(raw);
  }
}

export class Aex {
  readonly account: AccountClient;
  readonly organizations: OrganizationsClient;
  readonly workspaces: WorkspacesClient;
  readonly apiKeys: ApiKeysClient;
  readonly billing: BillingClient;
  readonly sessions: SessionsClient;
  readonly operations: OperationsClient;
  readonly workspace: WorkspaceClient;
  readonly events: ObservationSignalClient<"events">;
  readonly logs: ObservationSignalClient<"logs">;
  readonly spans: ObservationSignalClient<"spans">;
  readonly metrics: MetricsClient;
  readonly traces: TracesClient;
  readonly telemetry: TelemetryClient;

  constructor(apiKey: string, options?: Omit<AexOptions, "apiKey">);
  constructor(options: AexOptions);
  constructor(
    options: string | AexOptions,
    overrides: Omit<AexOptions, "apiKey"> = {}
  ) {
    const resolved = typeof options === "string"
      ? { ...overrides, apiKey: options }
      : options;
    const apiKey = resolved.apiKey;
    if (!apiKey) {
      throw new CredentialValidationError("Aex: apiKey is required");
    }
    const debug = resolved.debug === true
      ? (line: string) => console.error(line)
      : resolved.debug || undefined;
    const baseUrl =
      resolved.baseUrl ?? regionalBaseUrl(apiKey) ?? AEX_DEFAULT_BASE_URL;
    const bootstrapBaseUrl = resolved.bootstrapBaseUrl ??
      (resolved.baseUrl === undefined ? AEX_DEFAULT_BASE_URL : resolved.baseUrl);
    const fetchImpl = resolved.fetch ??
      ((input: RequestInfo | URL, init?: RequestInit) => fetch(input, init));
    const retry = resolved.retry === false
      ? false
      : resolved.retry ?? HTTP_RETRY_POLICY;
    const httpFor = (url: string) => new HttpClient({
      apiKey,
      baseUrl: url,
      fetch: fetchImpl,
      ...(debug ? { debug } : {}),
      retry
    });
    const http = httpFor(baseUrl);
    const bootstrapHttp = httpFor(bootstrapBaseUrl);
    const raw = new RawTelemetryTransport(baseUrl, apiKey, fetchImpl);
    this.account = new AccountClient(bootstrapHttp);
    this.organizations = new OrganizationsClient(bootstrapHttp);
    this.workspaces = new WorkspacesClient(bootstrapHttp);
    this.apiKeys = new ApiKeysClient(bootstrapHttp);
    this.billing = new BillingClient(
      bootstrapHttp,
      http,
      this.workspaces,
      httpFor
    );
    this.sessions = new SessionsClient(http, raw);
    this.operations = new OperationsClient(http);
    this.workspace = new WorkspaceClient(http);
    this.events = new ObservationSignalClient(http, raw, "events");
    this.logs = new ObservationSignalClient(http, raw, "logs");
    this.spans = new ObservationSignalClient(http, raw, "spans");
    this.metrics = new MetricsClient(http, raw);
    this.traces = new TracesClient(http, raw);
    this.telemetry = new TelemetryClient(http, raw);
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

function assertLimitId(value: string): string {
  if (!/^[a-z][a-z0-9_.-]{0,127}$/.test(value)) {
    throw new Error("limitId must be a lowercase dotted limit identifier");
  }
  return value;
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

function mergeUsageFrontiers(
  frontiers: readonly UsageFrontier[]
): readonly UsageFrontier[] {
  const merged = new Map<string, UsageFrontier>();
  for (const frontier of frontiers) {
    const key = `${frontier.region}\u0000${frontier.workspaceId}\u0000${frontier.category}`;
    const current = merged.get(key);
    if (
      current !== undefined &&
      JSON.stringify(current) !== JSON.stringify(frontier)
    ) {
      throw new Error(
        `Organization usage returned conflicting pinned frontiers for ${frontier.workspaceId}/${frontier.category}`
      );
    }
    merged.set(key, frontier);
  }
  return [...merged.values()];
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

function telemetryErrorMessage(body: unknown, status: number): string {
  if (
    typeof body === "object" &&
    body !== null &&
    "error" in body &&
    typeof body.error === "object" &&
    body.error !== null &&
    "message" in body.error &&
    typeof body.error.message === "string"
  ) {
    return body.error.message;
  }
  return `Aex telemetry request failed with status ${status}`;
}

async function* ndjsonLines(response: Response): AsyncGenerator<string, void, void> {
  if (response.body === null) {
    throw new TelemetryStreamProtocolError(
      "Telemetry stream response did not include a body"
    );
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      pending += decoder.decode(value, { stream: true });
      let lineEnd = pending.indexOf("\n");
      while (lineEnd !== -1) {
        const line = pending.slice(0, lineEnd).replace(/\r$/, "");
        pending = pending.slice(lineEnd + 1);
        if (line.trim().length > 0) yield line;
        lineEnd = pending.indexOf("\n");
      }
    }
    pending += decoder.decode();
    const finalLine = pending.replace(/\r$/, "");
    if (finalLine.trim().length > 0) yield finalLine;
  } finally {
    reader.releaseLock();
  }
}
