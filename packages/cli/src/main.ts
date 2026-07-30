import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { newId } from "@aexhq/contracts";
import {
  Aex,
  AexApiError,
  coordinateDownloadGrants,
  type DownloadGrant,
  type DownloadRange,
  type Id,
  type OperationHandle,
  type OperationKind,
  type OrganizationUsageQuery,
  type UsageQuery
} from "@aexhq/sdk";
import type { CliIO } from "./internal.js";

const OK = 0;
const FAILED = 1;
const USAGE = 2;
const TIMEOUT = 3;

const BOOLEAN_FLAGS = new Set([
  "detach", "force", "resume", "cascade", "debug", "help"
]);
const ALLOWED_FLAGS = new Set([
  ...BOOLEAN_FLAGS,
  "api-key", "aex-url", "bootstrap-url", "request", "query", "output",
  "idempotency-key", "operation-id", "if-revision", "confirm", "workspace",
  "organization", "session", "status", "kind", "cursor", "limit", "wake",
  "consistency", "range", "timeout-ms", "poll-interval-ms"
]);
const REMOVED_FLAGS = new Set([
  "from", "follow", "runtime", "runtime-size", "checkpoint", "webhook",
  "archive", "ticket", "version", "copy"
]);
const REMOVED = new Set([
  "start", "status", "deliveries", "wait", "cancel", "archive", "checkpoint",
  "webhook", "webhooks", "runtime", "runtime-sizes", "version", "copy",
  "resume", "suspend", "otel", "tail", "inspect", "download", "agents"
]);

interface Parsed {
  readonly words: readonly string[];
  readonly flags: ReadonlyMap<string, string | true>;
}

interface Context {
  readonly io: CliIO;
  readonly api: Aex;
  readonly apiKey: string;
  readonly baseUrl: string;
  readonly parsed: Parsed;
}

class UsageError extends Error {}

export async function executeCli(io: CliIO): Promise<void> {
  try {
    const parsed = parse(io.argv.slice(2));
    if (parsed.words.length === 0 || parsed.words[0] === "help" || has(parsed, "help")) {
      io.stdout(help());
      io.exit(OK);
      return;
    }
    if (parsed.words[0] === "--help" || parsed.words[0] === "-h") {
      io.stdout(help());
      io.exit(OK);
      return;
    }
    const top = parsed.words[0]!;
    if (REMOVED.has(top)) {
      throw new UsageError(`removed command: ${top}`);
    }
    const stored = await io.configStore?.read();
    const apiKey = flag(parsed, "api-key") ?? stored?.apiKey;
    if (!apiKey) throw new UsageError("--api-key is required");
    const baseUrl = flag(parsed, "aex-url") ?? stored?.aexUrl ?? "https://api.aex.dev";
    const bootstrapBaseUrl = flag(parsed, "bootstrap-url") ?? baseUrl;
    const api = new Aex({
      apiKey,
      baseUrl,
      bootstrapBaseUrl,
      fetch: io.fetchImpl,
      retry: false,
      debug: has(parsed, "debug") ? (line) => io.stderr(`${line}\n`) : false
    });
    const ctx: Context = { io, api, apiKey, baseUrl, parsed };
    const value = await dispatch(ctx);
    if (value !== undefined) await emit(ctx, value);
    io.exit(OK);
  } catch (error) {
    if (error instanceof UsageError) {
      io.stderr(`${error.message}\nuse \`aex help\` for usage\n`);
      io.exit(USAGE);
      return;
    }
    if (error instanceof AexApiError) {
      io.stderr(`${JSON.stringify(error.body)}\n`);
      io.exit(FAILED);
      return;
    }
    const message = error instanceof Error ? error.message : String(error);
    io.stderr(`${JSON.stringify({ error: "cli_error", message })}\n`);
    io.exit(/timed out/i.test(message) ? TIMEOUT : FAILED);
  }
}

async function dispatch(ctx: Context): Promise<unknown> {
  const [group, action, ...args] = ctx.parsed.words;
  switch (group) {
    case "account":
      requireAction(action, "get");
      return ctx.api.account.get(optionalOrganization(ctx));
    case "organizations":
      return organizations(ctx, action, args);
    case "workspaces":
      return workspaces(ctx, action, args);
    case "api-keys":
      return apiKeys(ctx, action, args);
    case "sessions":
      return sessions(ctx, action, args);
    case "messages":
      return messages(ctx, action, args);
    case "runs":
      return runs(ctx, action, args);
    case "operations":
      return operations(ctx, action, args);
    case "workspace":
      return workspace(ctx, action, args);
    case "files":
      return files(ctx, action, args);
    case "approvals":
      return approvals(ctx, action, args);
    case "events":
    case "logs":
    case "spans":
    case "metrics":
    case "traces":
      return signal(ctx, group, action, args);
    case "telemetry":
      return telemetry(ctx, action, args);
    case "billing":
      return billing(ctx, action, args);
    default:
      throw new UsageError(`unknown command: ${group ?? ""}`);
  }
}

async function organizations(ctx: Context, action: string | undefined, args: readonly string[]) {
  if (action === "list") return ctx.api.organizations.list(pageFlags(ctx));
  if (action === "create") {
    return ctx.api.organizations.create(await request(ctx), idempotency(ctx));
  }
  if (action === "get") return ctx.api.organizations.get(arg(args, 0, "organizationId"));
  if (action === "memberships" && args[0] === "list") {
    return ctx.api.organizations.memberships.list(arg(args, 1, "organizationId"), pageFlags(ctx));
  }
  if (action === "invitations" && args[0] === "create") {
    return ctx.api.organizations.invitations.create(
      arg(args, 1, "organizationId"),
      await request(ctx),
      idempotency(ctx)
    );
  }
  throw new UsageError("usage: aex organizations list|create|get|memberships list|invitations create");
}

async function workspaces(ctx: Context, action: string | undefined, args: readonly string[]) {
  if (action === "list") {
    return ctx.api.workspaces.list({ ...pageFlags(ctx), ...optionalOrganization(ctx) });
  }
  if (action === "create") {
    return ctx.api.workspaces.create(await request(ctx), idempotency(ctx));
  }
  if (action === "get") return ctx.api.workspaces.get(arg(args, 0, "workspaceId"));
  if (action === "delete") {
    const workspaceId = arg(args, 0, "workspaceId");
    const handle = await ctx.api.workspaces.delete(workspaceId, {
      confirmation: requiredFlag(ctx, "confirm"),
      ...operationAdmission(ctx)
    });
    return finishOperation(ctx, handle);
  }
  throw new UsageError("usage: aex workspaces list|create|get|delete");
}

async function apiKeys(ctx: Context, action: string | undefined, args: readonly string[]) {
  if (action === "list") {
    return ctx.api.apiKeys.list({
      workspaceId: requiredFlag(ctx, "workspace"),
      ...pageFlags(ctx)
    });
  }
  if (action === "create") return ctx.api.apiKeys.create(await request(ctx), idempotency(ctx));
  if (action === "revoke") {
    await ctx.api.apiKeys.revoke(arg(args, 0, "apiKeyId"), revision(ctx));
    return { revoked: true };
  }
  throw new UsageError("usage: aex api-keys list|create|revoke");
}

async function sessions(ctx: Context, action: string | undefined, args: readonly string[]) {
  if (action === "create") {
    const handle = await ctx.api.sessions.create(await request(ctx), idempotency(ctx));
    return handle.record;
  }
  if (action === "list") {
    return ctx.api.sessions.list({
      ...pageFlags(ctx),
      ...(flag(ctx.parsed, "status") ? { status: flag(ctx.parsed, "status") } : {})
    } as Parameters<typeof ctx.api.sessions.list>[0]);
  }
  if (action === "credentials" && args[0] === "rebind") {
    return sessionOperation(
      ctx,
      arg(args, 1, "sessionId"),
      "credential-rebinds",
      await request(ctx)
    );
  }
  const sessionId = arg(args, 0, "sessionId");
  if (action === "get") return ctx.api.sessions.get(sessionId);
  if (action === "stop") return sessionOperation(ctx, sessionId, "stops", {});
  if (action === "persist") {
    return sessionOperation(ctx, sessionId, "persists", await request(ctx, {}), true);
  }
  if (action === "fork") {
    return sessionOperation(ctx, sessionId, "forks", await request(ctx), true);
  }
  if (action === "delete") {
    return sessionOperation(
      ctx,
      sessionId,
      "deletions",
      { cascade: has(ctx.parsed, "cascade") }
    );
  }
  throw new UsageError("usage: aex sessions create|list|get|stop|persist|fork|delete|credentials rebind");
}

async function messages(ctx: Context, action: string | undefined, args: readonly string[]) {
  const sessionId = arg(args, 0, "sessionId");
  if (action === "list") {
    return regional(
      ctx,
      withPage(`/api/sessions/${encodeURIComponent(sessionId)}/messages`, pageFlags(ctx)),
      "GET"
    );
  }
  if (action === "send") {
    const session = await ctx.api.sessions.open(sessionId);
    const accepted = await session.messages.send(await request(ctx), idempotency(ctx));
    return { message: accepted.message, run: accepted.run.record };
  }
  throw new UsageError("usage: aex messages list|send <sessionId>");
}

async function runs(ctx: Context, action: string | undefined, args: readonly string[]) {
  const sessionId = arg(args, 0, "sessionId");
  if (action === "list") {
    return regional(
      ctx,
      withPage(`/api/sessions/${encodeURIComponent(sessionId)}/runs`, pageFlags(ctx)),
      "GET"
    );
  }
  if (action === "get") {
    const runId = arg(args, 1, "runId");
    return regional(
      ctx,
      `/api/sessions/${encodeURIComponent(sessionId)}/runs/${encodeURIComponent(runId)}`,
      "GET"
    );
  }
  throw new UsageError("usage: aex runs list|get <sessionId> [runId]");
}

async function operations(ctx: Context, action: string | undefined, args: readonly string[]) {
  if (action === "list") {
    return ctx.api.operations.list({
      ...pageFlags(ctx),
      ...(flag(ctx.parsed, "session") ? { sessionId: flag(ctx.parsed, "session") } : {}),
      ...(flag(ctx.parsed, "kind") ? { kind: flag(ctx.parsed, "kind") } : {}),
      ...(flag(ctx.parsed, "status") ? { status: flag(ctx.parsed, "status") } : {})
    } as Parameters<typeof ctx.api.operations.list>[0]);
  }
  const operationId = arg(args, 0, "operationId");
  if (action === "get") return ctx.api.operations.get(operationId);
  if (action === "wait") {
    return (await ctx.api.operations.open(operationId)).wait(waitOptions(ctx));
  }
  if (action === "cancel") return ctx.api.operations.cancel(operationId);
  throw new UsageError("usage: aex operations list|get|wait|cancel");
}

async function workspace(ctx: Context, action: string | undefined, args: readonly string[]) {
  if (action === "get") return ctx.api.workspace.get();
  if (action === "discard") {
    return sessionOperation(
      ctx,
      arg(args, 0, "sessionId"),
      "workspace/discards",
      await request(ctx, {})
    );
  }
  if (action === "limits") {
    const subcommand = arg(args, 0, "action");
    if (subcommand === "list") {
      return ctx.api.workspace.limits.list(pageFlags(ctx));
    }
    if (subcommand === "get") {
      return ctx.api.workspace.limits.get(arg(args, 1, "limitId"));
    }
    throw new UsageError("workspace limits support list|get");
  }
  const registry = {
    files: ctx.api.workspace.files,
    skills: ctx.api.workspace.skills,
    tools: ctx.api.workspace.tools,
    instructions: ctx.api.workspace.instructions,
    "mcp-servers": ctx.api.workspace.mcpServers
  }[action ?? ""];
  if (registry) return registered(ctx, registry, args);
  if (action === "secrets") return secrets(ctx, args);
  if (action === "uploads") return uploads(ctx, args);
  throw new UsageError("usage: aex workspace get|limits|discard|files|skills|tools|instructions|mcp-servers|secrets|uploads");
}

interface RegistryLike {
  list(query?: { cursor?: string; limit?: number }): Promise<unknown>;
  get(name: string): Promise<unknown>;
  set(name: string, value: never, options?: { idempotencyKey?: string; ifRevision?: number }): Promise<unknown>;
  delete(name: string, options?: { ifRevision?: number }): Promise<void>;
  download?(
    name: string,
    request?: { range?: { start: number; endExclusive: number } },
    options?: { idempotencyKey?: string }
  ): Promise<DownloadGrant>;
}

async function registered(ctx: Context, client: RegistryLike, args: readonly string[]) {
  const action = arg(args, 0, "action");
  if (action === "list") return client.list(pageFlags(ctx));
  const name = arg(args, 1, "name");
  if (action === "get") return client.get(name);
  if (action === "set") {
    return client.set(name, await request(ctx) as never, { ...idempotency(ctx), ...revision(ctx) });
  }
  if (action === "delete") {
    await client.delete(name, revision(ctx));
    return { deleted: true };
  }
  if (action === "download") {
    if (client.download === undefined) {
      throw new UsageError("only registered files support download");
    }
    await preflightDownload(ctx);
    const descriptor = registeredFileDescriptor(await client.get(name));
    await consumeRangedDownload(
      ctx,
      descriptor,
      rangeRequest(ctx).range,
      (range, options) => client.download!(name, { range }, options)
    );
    return undefined;
  }
  throw new UsageError("registered resources support list|get|set|delete; files also support download");
}

function registeredFileDescriptor(resource: unknown): DownloadDescriptor {
  const content = (
    resource as {
      readonly value?: {
        readonly content?: {
          readonly sizeBytes?: unknown;
          readonly sha256?: unknown;
        };
      };
    }
  ).value?.content;
  const size = content?.sizeBytes;
  if (typeof size !== "number" || !Number.isSafeInteger(size) || size < 0) {
    throw new Error("registered file response has no valid content size");
  }
  const sha256 = content?.sha256;
  if (typeof sha256 !== "string" || !/^sha256:[0-9a-f]{64}$/.test(sha256)) {
    throw new Error("registered file response has no valid content SHA-256");
  }
  return { sizeBytes: size, sha256 };
}

async function secrets(ctx: Context, args: readonly string[]) {
  const action = arg(args, 0, "action");
  if (action === "list") return ctx.api.workspace.secrets.list(pageFlags(ctx));
  const name = arg(args, 1, "name");
  if (action === "get") return ctx.api.workspace.secrets.get(name);
  if (action === "set") {
    const body = await request(ctx) as { value?: unknown };
    if (typeof body.value !== "string") throw new UsageError("secret set request requires string value");
    return ctx.api.workspace.secrets.set(name, body.value, { ...idempotency(ctx), ...revision(ctx) });
  }
  if (action === "delete") {
    await ctx.api.workspace.secrets.delete(name);
    return { deleted: true };
  }
  if (action === "revoke") return ctx.api.workspace.secrets.revoke(name, idempotency(ctx));
  throw new UsageError("secrets support list|get|set|delete|revoke");
}

async function uploads(ctx: Context, args: readonly string[]) {
  const action = arg(args, 0, "action");
  if (action === "create") {
    return ctx.api.workspace.uploads.create(await request(ctx), idempotency(ctx));
  }
  const uploadId = arg(args, 1, "uploadId");
  if (action === "parts") {
    return ctx.api.workspace.uploads.parts(uploadId, await request(ctx));
  }
  if (action === "complete") {
    return ctx.api.workspace.uploads.complete(uploadId, await request(ctx), idempotency(ctx));
  }
  if (action === "abort") {
    await ctx.api.workspace.uploads.abort(uploadId);
    return { aborted: true };
  }
  throw new UsageError("uploads support create|parts|complete|abort");
}

async function files(ctx: Context, scope: string | undefined, args: readonly string[]) {
  if (scope !== "live" && scope !== "persisted") {
    throw new UsageError("usage: aex files live|persisted list|stat|download");
  }
  const action = arg(args, 0, "action");
  const sessionId = arg(args, 1, "sessionId");
  if (scope === "live") validateLiveFlags(ctx);
  if (action === "download") await preflightDownload(ctx);
  const session = await ctx.api.sessions.open(sessionId);
  const client = scope === "live" ? session.files.live : session.files.persisted;
  if (action === "list") return client.list(await fileRequest(ctx, scope));
  if (action === "stat") {
    return client.stat({ path: arg(args, 2, "path"), ...await fileRequest(ctx, scope) });
  }
  if (action === "download") {
    const path = arg(args, 2, "path");
    let access = await fileRequest(ctx, scope);
    const entry = await client.stat({ path, ...access } as never);
    if (entry.type !== "file") throw new UsageError("only files can be downloaded");
    if (entry.sha256 === undefined) {
      throw new Error("file response has no declared SHA-256");
    }
    if (scope === "live" && !("ifGenerationId" in access)) {
      access = {
        ...access,
        ifGenerationId: (
          entry as unknown as {
            readonly workspaceAccess: { readonly generationId: string };
          }
        ).workspaceAccess.generationId
      };
    }
    await consumeRangedDownload(
      ctx,
      {
        sizeBytes: entry.sizeBytes,
        sha256: entry.sha256
      },
      rangeRequest(ctx).range,
      (range, options) => client.download(
        { path, ...access, range } as never,
        options
      )
    );
    return undefined;
  }
  throw new UsageError("files support list|stat|download");
}

async function approvals(ctx: Context, action: string | undefined, args: readonly string[]) {
  const session = await ctx.api.sessions.open(arg(args, 0, "sessionId"));
  if (action === "list") return session.approvals.list(pageFlags(ctx));
  const approvalId = arg(args, 1, "approvalId");
  if (action === "get") return session.approvals.get(approvalId);
  if (action === "respond") return session.approvals.respond(approvalId, await request(ctx));
  throw new UsageError("usage: aex approvals list|get|respond <sessionId>");
}

async function signal(
  ctx: Context,
  group: "events" | "logs" | "spans" | "metrics" | "traces",
  action: string | undefined,
  args: readonly string[]
) {
  const sessionId = flag(ctx.parsed, "session");
  const owner = sessionId ? await ctx.api.sessions.open(sessionId) : ctx.api;
  const client = owner[group];
  if (action === "query") return client.query(await query(ctx));
  if (action === "stream" || action === "listen") {
    const iterable = action === "stream"
      ? client.stream(await query(ctx) as never)
      : client.listen(await query(ctx));
    for await (const frame of iterable) ctx.io.stdout(`${JSON.stringify(frame)}\n`);
    return undefined;
  }
  if (group === "metrics" && action === "aggregate") {
    return owner.metrics.aggregate(await query(ctx));
  }
  if (group === "traces" && action === "get") {
    if (!sessionId) throw new UsageError("traces get requires --session");
    return owner.traces.get(arg(args, 0, "traceId"));
  }
  throw new UsageError(`${group} supports query|stream|listen${group === "metrics" ? "|aggregate" : ""}`);
}

async function telemetry(ctx: Context, action: string | undefined, args: readonly string[]) {
  const sessionId = flag(ctx.parsed, "session");
  const owner = sessionId ? await ctx.api.sessions.open(sessionId) : ctx.api;
  const client = owner.telemetry;
  if (action === "query") return client.query(await query(ctx));
  if (action === "stream" || action === "listen") {
    const iterable = action === "stream"
      ? client.stream(await query(ctx) as never)
      : client.listen(await query(ctx));
    for await (const frame of iterable) ctx.io.stdout(`${JSON.stringify(frame)}\n`);
    return undefined;
  }
  if (action === "gaps") {
    if (args[0] === "query") return client.gaps.query(await query(ctx));
    if (args[0] === "get") return client.gaps.get(arg(args, 1, "gapId"));
  }
  if (action === "export") {
    return finishOperation(ctx, await client.export(await request(ctx), operationAdmission(ctx)));
  }
  if (action === "exports" && args[0] === "get") {
    return client.exports.get(arg(args, 1, "exportId"));
  }
  if (action === "download") {
    await preflightDownload(ctx);
    const exportId = arg(args, 0, "exportId");
    const record = await client.exports.get(exportId);
    if (record.status !== "ready") throw new UsageError("telemetry export is not ready");
    await consumeDownload(ctx, await client.exports.download(exportId, idempotency(ctx)));
    return undefined;
  }
  if (action === "revoke") {
    return client.exports.revoke(arg(args, 0, "exportId"), idempotency(ctx));
  }
  throw new UsageError("telemetry supports query|stream|listen|gaps|export|exports get|download|revoke");
}

async function billing(ctx: Context, action: string | undefined, args: readonly string[]) {
  if (action === "balance") return ctx.api.billing.balance.get(optionalOrganization(ctx));
  if (action === "usage") {
    const organizationId = flag(ctx.parsed, "organization");
    return organizationId
      ? ctx.api.billing.usage.queryOrganization(
        organizationId,
        await query<OrganizationUsageQuery>(ctx)
      )
      : ctx.api.billing.usage.query(await query<UsageQuery>(ctx));
  }
  if (action === "top-up") {
    return ctx.api.billing.topUpCheckout(
      arg(args, 0, "organizationId"),
      await request(ctx),
      idempotency(ctx)
    );
  }
  if (action === "portal") {
    return ctx.api.billing.portalSession(
      arg(args, 0, "organizationId"),
      await request(ctx),
      idempotency(ctx)
    );
  }
  if (action === "auto-topup") {
    const sub = arg(args, 0, "action");
    const organizationId = arg(args, 1, "organizationId");
    if (sub === "get") return ctx.api.billing.autoTopup.get(organizationId);
    if (sub === "replace") {
      const ifRevision = numericFlag(ctx, "if-revision", true)!;
      return ctx.api.billing.autoTopup.replace(
        organizationId,
        await request(ctx),
        { ...idempotency(ctx), ifRevision }
      );
    }
  }
  if (action === "statements") {
    const sub = arg(args, 0, "action");
    const organizationId = requiredFlag(ctx, "organization");
    if (sub === "list") return ctx.api.billing.statements.list(organizationId, pageFlags(ctx));
    const statementId = arg(args, 1, "statementId");
    if (sub === "get") return ctx.api.billing.statements.get(organizationId, statementId);
    if (sub === "download") {
      await preflightDownload(ctx);
      await consumeDownload(
        ctx,
        await ctx.api.billing.statements.download(organizationId, statementId, idempotency(ctx))
      );
      return undefined;
    }
  }
  throw new UsageError("billing supports balance|usage|top-up|portal|auto-topup|statements");
}

async function finishOperation<K extends OperationKind>(
  ctx: Context,
  handle: OperationHandle<K>
): Promise<unknown> {
  if (has(ctx.parsed, "detach")) return handle.record;
  return handle.wait(waitOptions(ctx));
}

async function regional(
  ctx: Context,
  path: string,
  method: "GET" | "POST",
  body?: unknown,
  headers: Readonly<Record<string, string>> = {}
): Promise<unknown> {
  const response = await ctx.io.fetchImpl(new URL(path, ctx.baseUrl), {
    method,
    headers: {
      authorization: `Bearer ${ctx.apiKey}`,
      accept: "application/json",
      ...headers,
      ...(body === undefined ? {} : { "content-type": "application/json" })
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) })
  });
  if (!response.ok) {
    let parsed: unknown;
    try { parsed = await response.json(); } catch { parsed = { error: "http_error" }; }
    throw new AexApiError(response.status, `HTTP ${response.status}`, parsed);
  }
  return response.status === 204 ? undefined : response.json();
}

async function sessionOperation(
  ctx: Context,
  sessionId: string,
  route: string,
  body: unknown,
  withRevision = false
): Promise<unknown> {
  const operationId = flag(ctx.parsed, "operation-id") ?? newId("operation");
  const ifRevision = withRevision ? numericFlag(ctx, "if-revision") : undefined;
  const operation = await regional(
    ctx,
    `/api/sessions/${encodeURIComponent(sessionId)}/${route}`,
    "POST",
    body,
    {
      "Aex-Operation-Id": operationId,
      ...(ifRevision === undefined ? {} : { "If-Match": `"${ifRevision}"` })
    }
  ) as { readonly id?: unknown };
  if (operation.id !== operationId) {
    throw new Error(`operation admission returned a different operation id`);
  }
  if (has(ctx.parsed, "detach")) return operation;
  return (await ctx.api.operations.open(operationId)).wait(waitOptions(ctx));
}

async function consumeDownload(
  ctx: Context,
  grant: DownloadGrant,
  resumeSupported = false
): Promise<void> {
  const output = requiredFlag(ctx, "output");
  const resume = has(ctx.parsed, "resume");
  const force = has(ctx.parsed, "force");
  if (resume && !resumeSupported) {
    throw new UsageError("--resume is not supported by this grant endpoint");
  }
  if (output !== "-" && !ctx.io.renameFile) {
    throw new UsageError("this CLI host cannot atomically rename downloads");
  }
  const target = output === "-" ? "-" : resolve(ctx.io.cwd(), output);
  const part = target === "-" ? "-" : `${target}.part`;
  if (target !== "-" && await exists(ctx, target) && !force) {
    throw new UsageError(`output already exists: ${target}`);
  }
  if (target !== "-" && (!resume || !await exists(ctx, part))) {
    await ctx.io.writeFile(part, new Uint8Array());
  }
  if (target !== "-" && !ctx.io.appendFile) {
    throw new UsageError("this CLI host cannot stream file downloads");
  }
  const digest = await streamGrant(ctx, grant, async (chunk) => {
    if (target === "-") {
      if (!ctx.io.stdoutBytes) throw new UsageError("binary stdout is unavailable");
      ctx.io.stdoutBytes(chunk);
    } else {
      await ctx.io.appendFile!(part, chunk);
    }
  });
  if (digest !== grant.sha256) throw new Error("download SHA-256 mismatch");
  if (target === "-") return;
  await ctx.io.renameFile!(part, target);
  ctx.io.stderr(`downloaded ${grant.authorizedBytes} bytes to ${target}\n`);
}

interface DownloadDescriptor {
  readonly sizeBytes: number;
  readonly sha256: string;
}

type DownloadGrantMinter = (
  range: DownloadRange,
  options: { readonly idempotencyKey?: string }
) => Promise<DownloadGrant>;

async function consumeRangedDownload(
  ctx: Context,
  descriptor: DownloadDescriptor,
  selected: DownloadRange | undefined,
  mint: DownloadGrantMinter
): Promise<void> {
  const output = requiredFlag(ctx, "output");
  const resume = has(ctx.parsed, "resume");
  const force = has(ctx.parsed, "force");
  if (resume && output === "-") {
    throw new UsageError("--resume requires file output");
  }
  if (output !== "-" && (!ctx.io.renameFile || !ctx.io.appendFile || !ctx.io.fileSize)) {
    throw new UsageError(
      "this CLI host cannot stream, size, and atomically rename downloads"
    );
  }
  const target = output === "-" ? "-" : resolve(ctx.io.cwd(), output);
  const part = target === "-" ? "-" : `${target}.part`;
  if (target !== "-" && await exists(ctx, target) && !force) {
    throw new UsageError(`output already exists: ${target}`);
  }

  const selectionStart = selected?.start ?? 0;
  const selectionEnd = selected?.endExclusive ?? descriptor.sizeBytes;
  const selectedBytes = selectionEnd - selectionStart;
  let prefixBytes = 0;
  if (target !== "-") {
    if (resume) {
      if (!await exists(ctx, part)) {
        throw new UsageError("--resume requires an existing .part file");
      }
      prefixBytes = await ctx.io.fileSize!(part);
      if (prefixBytes >= selectedBytes) {
        throw new UsageError("partial file already covers the selected range");
      }
    } else {
      await ctx.io.writeFile(part, new Uint8Array());
    }
  }

  const remaining = prefixBytes === 0
    ? selected
    : {
        start: selectionStart + prefixBytes,
        endExclusive: selectionEnd
      };
  const wholeDigest = selected === undefined && !resume
    ? createHash("sha256")
    : undefined;
  let writtenBytes = prefixBytes;
  const coordinated = coordinateDownloadGrants({
    sizeBytes: descriptor.sizeBytes,
    sha256: descriptor.sha256,
    ...(remaining === undefined ? {} : { range: remaining }),
    mint: (range, position) => mint(
      range,
      rangeIdempotency(ctx, range, position.index, position.count)
    )
  });
  for await (const { grant, range } of coordinated) {
    const rangeDigest = await streamGrant(ctx, grant, async (chunk) => {
      wholeDigest?.update(chunk);
      writtenBytes += chunk.byteLength;
      if (target === "-") {
        if (!ctx.io.stdoutBytes) {
          throw new UsageError("binary stdout is unavailable");
        }
        ctx.io.stdoutBytes(chunk);
      } else {
        await ctx.io.appendFile!(part, chunk);
      }
    });
    if (
      range.start === 0 &&
      range.endExclusive === descriptor.sizeBytes &&
      rangeDigest !== grant.sha256
    ) {
      throw new Error("download SHA-256 mismatch");
    }
  }

  if (writtenBytes !== selectedBytes) {
    throw new Error(
      `download length mismatch: expected ${selectedBytes}, got ${writtenBytes}`
    );
  }
  if (selected === undefined) {
    const actual = resume
      ? await hashFile(ctx, part)
      : `sha256:${wholeDigest!.digest("hex")}`;
    if (actual !== descriptor.sha256) {
      throw new Error("complete download SHA-256 mismatch");
    }
  }
  if (target === "-") return;
  if (await ctx.io.fileSize!(part) !== selectedBytes) {
    throw new Error("partial-file length changed during download");
  }
  await ctx.io.renameFile!(part, target);
  ctx.io.stderr(`downloaded ${selectedBytes} bytes to ${target}\n`);
}

async function streamGrant(
  ctx: Context,
  grant: DownloadGrant,
  consume: (chunk: Uint8Array) => Promise<void>
): Promise<string> {
  const response = await ctx.io.fetchImpl(grant.url, {
    method: "GET",
    ...(grant.headers ? { headers: grant.headers } : {})
  });
  if (!response.ok) throw new Error(`download failed with HTTP ${response.status}`);
  if (!response.body) throw new Error("download response has no body");
  const digest = createHash("sha256");
  let receivedBytes = 0;
  const reader = response.body.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    const chunk = new Uint8Array(value);
    receivedBytes += chunk.byteLength;
    if (receivedBytes > grant.authorizedBytes) {
      await reader.cancel();
      throw new Error(
        `download length mismatch: expected ${grant.authorizedBytes}, got more`
      );
    }
    digest.update(chunk);
    await consume(chunk);
  }
  if (receivedBytes !== grant.authorizedBytes) {
    throw new Error(
      `download length mismatch: expected ${grant.authorizedBytes}, got ${receivedBytes}`
    );
  }
  return `sha256:${digest.digest("hex")}`;
}

function rangeIdempotency(
  ctx: Context,
  range: DownloadRange,
  index: number,
  count: number
): { readonly idempotencyKey?: string } {
  const base = flag(ctx.parsed, "idempotency-key");
  if (base === undefined || count === 1) {
    return base === undefined ? {} : { idempotencyKey: base };
  }
  const digest = createHash("sha256")
    .update(`${base}\0${index}\0${range.start}\0${range.endExclusive}`)
    .digest("hex");
  return { idempotencyKey: `range_${digest}` };
}

async function hashFile(ctx: Context, path: string): Promise<string> {
  if (!ctx.io.sha256File) {
    throw new UsageError("this CLI host cannot verify a resumed download");
  }
  return ctx.io.sha256File(path);
}

async function preflightDownload(ctx: Context): Promise<void> {
  const output = requiredFlag(ctx, "output");
  if (output === "-") return;
  const target = resolve(ctx.io.cwd(), output);
  if (await exists(ctx, target) && !has(ctx.parsed, "force")) {
    throw new UsageError(`output already exists: ${target}`);
  }
}

async function exists(ctx: Context, path: string): Promise<boolean> {
  if (ctx.io.fileExists) return ctx.io.fileExists(path);
  try {
    await ctx.io.readFileBytes!(path);
    return true;
  } catch {
    return false;
  }
}

async function fileRequest(ctx: Context, scope: "live" | "persisted") {
  const body = await request(ctx, {}) as Record<string, unknown>;
  if (scope === "persisted") return body;
  return {
    ...body,
    ...(flag(ctx.parsed, "wake") ? { wake: flag(ctx.parsed, "wake") } : {}),
    ...(flag(ctx.parsed, "consistency") ? { consistency: flag(ctx.parsed, "consistency") } : {})
  };
}

function rangeRequest(ctx: Context): { range?: { start: number; endExclusive: number } } {
  const value = flag(ctx.parsed, "range");
  if (!value) return {};
  const match = /^(\d+):(\d+)$/.exec(value);
  if (!match) throw new UsageError("--range must be start:end");
  const start = Number(match[1]);
  const endExclusive = Number(match[2]);
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(endExclusive) || endExclusive <= start) {
    throw new UsageError("--range must be a non-empty safe-integer range");
  }
  return { range: { start, endExclusive } };
}

async function query<T>(ctx: Context): Promise<T> {
  const source = flag(ctx.parsed, "query");
  if (source === undefined && ctx.io.stdinIsTTY === false) {
    return jsonInput(ctx, "-", {} as T);
  }
  return jsonInput(ctx, source, {} as T);
}

async function request<T>(ctx: Context, fallback?: T): Promise<T> {
  return jsonInput(ctx, flag(ctx.parsed, "request"), fallback);
}

async function jsonInput<T>(
  ctx: Context,
  source: string | undefined,
  fallback?: T
): Promise<T> {
  if (source === undefined) {
    if (fallback !== undefined) return fallback;
    throw new UsageError("--request is required");
  }
  let text: string;
  if (source === "-") {
    if (!ctx.io.readStdin) throw new UsageError("stdin is unavailable");
    text = await ctx.io.readStdin();
  } else if (source.startsWith("@")) {
    text = await ctx.io.readFile(resolve(ctx.io.cwd(), source.slice(1)));
  } else {
    text = source;
  }
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new UsageError("input must be valid JSON");
  }
}

async function emit(ctx: Context, value: unknown): Promise<void> {
  const text = `${JSON.stringify(value, null, 2)}\n`;
  const output = flag(ctx.parsed, "output");
  if (!output || output === "-") {
    ctx.io.stdout(text);
    return;
  }
  await ctx.io.writeFile(resolve(ctx.io.cwd(), output), new TextEncoder().encode(text));
}

function parse(args: readonly string[]): Parsed {
  const words: string[] = [];
  const flags = new Map<string, string | true>();
  for (let index = 0; index < args.length; index += 1) {
    const token = args[index]!;
    if (!token.startsWith("--")) {
      words.push(token);
      continue;
    }
    const equal = token.indexOf("=");
    const name = token.slice(2, equal === -1 ? undefined : equal);
    if (!name) throw new UsageError("invalid empty flag");
    if (REMOVED_FLAGS.has(name)) throw new UsageError(`removed flag: --${name}`);
    if (!ALLOWED_FLAGS.has(name)) throw new UsageError(`unknown flag: --${name}`);
    if (flags.has(name)) throw new UsageError(`duplicate flag: --${name}`);
    if (equal !== -1) {
      flags.set(name, token.slice(equal + 1));
    } else if (BOOLEAN_FLAGS.has(name)) {
      flags.set(name, true);
    } else {
      const value = args[index + 1];
      if (value === undefined || value.startsWith("--")) {
        throw new UsageError(`--${name} requires a value`);
      }
      flags.set(name, value);
      index += 1;
    }
  }
  return { words, flags };
}

function flag(parsed: Parsed, name: string): string | undefined {
  const value = parsed.flags.get(name);
  return typeof value === "string" ? value : undefined;
}

function has(parsed: Parsed, name: string): boolean {
  return parsed.flags.get(name) === true;
}

function requiredFlag(ctx: Context, name: string): string {
  const value = flag(ctx.parsed, name);
  if (!value) throw new UsageError(`--${name} is required`);
  return value;
}

function numericFlag(ctx: Context, name: string, required = false): number | undefined {
  const raw = flag(ctx.parsed, name);
  if (raw === undefined) {
    if (required) throw new UsageError(`--${name} is required`);
    return undefined;
  }
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value < 0) throw new UsageError(`--${name} must be a non-negative integer`);
  return value;
}

function arg(args: readonly string[], index: number, name: string): string {
  const value = args[index];
  if (!value) throw new UsageError(`${name} is required`);
  return value;
}

function requireAction(actual: string | undefined, expected: string): void {
  if (actual !== expected) throw new UsageError(`expected action: ${expected}`);
}

function pageFlags(ctx: Context): { cursor?: string; limit?: number } {
  const cursor = flag(ctx.parsed, "cursor");
  const limit = numericFlag(ctx, "limit");
  return {
    ...(cursor ? { cursor } : {}),
    ...(limit !== undefined ? { limit } : {})
  };
}

function withPage(path: string, query: { cursor?: string; limit?: number }): string {
  const search = new URLSearchParams();
  if (query.cursor !== undefined) search.set("cursor", query.cursor);
  if (query.limit !== undefined) search.set("limit", String(query.limit));
  const suffix = search.toString();
  return suffix ? `${path}?${suffix}` : path;
}

function idempotency(ctx: Context): { idempotencyKey?: string } {
  const value = flag(ctx.parsed, "idempotency-key");
  return value ? { idempotencyKey: value } : {};
}

function revision(ctx: Context): { ifRevision?: number } {
  const value = numericFlag(ctx, "if-revision");
  return value === undefined ? {} : { ifRevision: value };
}

function operationAdmission(ctx: Context): { operationId?: Id<"operation"> } {
  const value = flag(ctx.parsed, "operation-id");
  return value ? { operationId: value as Id<"operation"> } : {};
}

function optionalOrganization(ctx: Context): { organizationId?: string } {
  const value = flag(ctx.parsed, "organization");
  return value ? { organizationId: value } : {};
}

function waitOptions(ctx: Context): { timeoutMs?: number; pollIntervalMs?: number } {
  const timeoutMs = numericFlag(ctx, "timeout-ms");
  const pollIntervalMs = numericFlag(ctx, "poll-interval-ms");
  return {
    ...(timeoutMs !== undefined ? { timeoutMs } : {}),
    ...(pollIntervalMs !== undefined ? { pollIntervalMs } : {})
  };
}

function validateLiveFlags(ctx: Context): void {
  const wake = flag(ctx.parsed, "wake");
  if (wake !== undefined && wake !== "retained" && wake !== "never") {
    throw new UsageError("--wake must be retained or never");
  }
  const consistency = flag(ctx.parsed, "consistency");
  if (
    consistency !== undefined &&
    consistency !== "coherent" &&
    consistency !== "best_effort"
  ) {
    throw new UsageError("--consistency must be coherent or best_effort");
  }
}

function help(): string {
  return `aex — explicit v1 session CLI

Usage:
  aex account get
  aex organizations list|create|get|memberships list|invitations create
  aex workspaces list|create|get|delete
  aex api-keys list|create|revoke
  aex sessions create|list|get|stop|persist|fork|delete|credentials rebind
  aex messages list|send <sessionId>
  aex runs list|get <sessionId> [runId]
  aex operations list|get|wait|cancel
  aex workspace get|limits list|get|discard|files|skills|tools|instructions|mcp-servers|secrets|uploads
  aex workspace files download <name> --output <file|->
  aex files live|persisted list|stat|download
  aex approvals list|get|respond
  aex events|logs|spans|metrics|traces query|stream|listen [--session ID]
  aex telemetry query|stream|listen|gaps|export|download|revoke [--session ID]
  aex billing balance|usage|top-up|portal|auto-topup|statements

JSON input:
  --request <json|@file|->   mutation body
  --query <json|@file|->     observational query body

Common:
  --api-key KEY --aex-url URL --idempotency-key KEY --operation-id OP
  --if-revision N --detach --timeout-ms N --poll-interval-ms N
  --output <file|-> --force --resume
`;
}
