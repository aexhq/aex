import { afterAll, expect, test } from "bun:test";
import { createHash, randomUUID } from "node:crypto";
import { writeFileSync } from "node:fs";
import { newId } from "@aexhq/sdk";
import { USER_SCENARIOS } from "../../scenarios.js";

const createdFiles = new Set<string>();
const createdSessions = new Set<string>();

test("live scenarios fail closed onto dev descriptors", () => {
  for (const row of USER_SCENARIOS.filter(({ suite }) => suite === "live")) {
    expect(row.plane).toBe("dev");
  }
});

test.serial("live.files-inline-overwrite-download-delete", async () => {
  if (inventory()) return;
  const name = unique("inline");
  createdFiles.add(name);
  const binary = Uint8Array.from([0, 255, 1, 254, 2, 253, 3, 252]);
  const first = await putInline(name, Uint8Array.from([1, 2, 3]), "application/octet-stream");
  const replacement = await putInline(name, binary, "application/octet-stream");
  expect(first.value.content.sha256).not.toBe(replacement.value.content.sha256);
  expect(replacement.value.content.sha256).toBe(digest(binary));

  const grant = await jsonRequest<DownloadGrant>(`/api/files/${encodeURIComponent(name)}/downloads`, {
    method: "POST",
    body: {},
    idempotencyKey: randomUUID(),
  });
  const downloaded = new Uint8Array(await (await fetch(grant.url, {
    signal: AbortSignal.timeout(30_000),
  })).arrayBuffer());
  expect(downloaded).toEqual(binary);
  expect(digest(downloaded)).toBe(grant.sha256);

  await request(`/api/files/${encodeURIComponent(name)}`, { method: "DELETE" }, 204);
  createdFiles.delete(name);
  await request(`/api/files/${encodeURIComponent(name)}`, {}, 404);
}, 90_000);

test.serial("live.files-url-import-is-latest-and-never-falls-back", async () => {
  if (inventory()) return;
  const source = unique("url-source");
  const target = unique("url-target");
  createdFiles.add(source);
  createdFiles.add(target);
  const bytes = new TextEncoder().encode(`url-import-${randomUUID()}`);
  await putInline(source, bytes, "text/plain");
  const sourceGrant = await jsonRequest<DownloadGrant>(
    `/api/files/${encodeURIComponent(source)}/downloads`,
    { method: "POST", body: {}, idempotencyKey: randomUUID() },
  );

  const admitted = await jsonRequest<WorkspaceFile>(`/api/files/${encodeURIComponent(target)}`, {
    method: "PUT",
    idempotencyKey: randomUUID(),
    body: {
      content: { type: "url", url: sourceGrant.url },
      mediaType: "text/plain",
      mode: "0644",
    },
  });
  expect(["pending", "ready"]).toContain(admitted.state);
  const imported = await waitForReady(target);
  expect(imported.value.content.sha256).toBe(digest(bytes));
}, 120_000);

test.serial("live.files-direct-upload-publishes-arbitrary-binary", async () => {
  if (inventory()) return;
  const name = unique("upload");
  createdFiles.add(name);
  const bytes = Uint8Array.from({ length: 65_537 }, (_, index) => (index * 131) % 256);
  const admission = await jsonRequest<UploadAdmission>("/api/uploads", {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      name,
      sizeBytes: String(bytes.byteLength),
      sha256: digest(bytes),
      contentType: "application/octet-stream",
      parts: [{ partNumber: 1, sha256: digest(bytes), sizeBytes: String(bytes.byteLength) }],
    },
  });
  expect(admission.grants.length).toBe(admission.upload.partCount);
  const parts: Array<{ partNumber: number; etag: string }> = [];
  for (const [index, grant] of admission.grants.entries()) {
    const start = index * Number(admission.upload.partSizeBytes);
    const end = Math.min(start + Number(admission.upload.partSizeBytes), bytes.byteLength);
    const headers = new Headers(grant.headers.map(({ name, value }) => [name, value]));
    const response = await fetch(grant.url, {
      method: "PUT",
      headers,
      body: bytes.slice(start, end),
      signal: AbortSignal.timeout(60_000),
    });
    expect(response.ok).toBe(true);
    const etag = response.headers.get("etag");
    if (!etag) throw new Error("object store omitted the multipart ETag");
    parts.push({ partNumber: grant.partNumber, etag });
  }
  const published = await jsonRequest<ReadyWorkspaceFile>(
    `/api/uploads/${encodeURIComponent(admission.upload.id)}/completions`,
    { method: "POST", idempotencyKey: randomUUID(), body: { parts } },
  );
  expect(published.name).toBe(name);
  expect(published.value.content.sha256).toBe(digest(bytes));
}, 180_000);

test.serial("live.files-session-mount-freezes-content", async () => {
  if (inventory()) return;
  const name = unique("mount");
  createdFiles.add(name);
  const original = new TextEncoder().encode(`frozen-${randomUUID()}`);
  const replacement = new TextEncoder().encode(`new-${randomUUID()}`);
  await putInline(name, original, "text/plain");
  const session = await createSession(name);
  createdSessions.add(session.id);
  await putInline(name, replacement, "text/plain");

  await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      text: `Use read_file on /workspace/inputs/frozen.txt. Reply with only its exact contents. Expected content begins frozen-.`,
    },
  });
  const transcript = await waitForAssistantText(session.id);
  expect(transcript).toContain(new TextDecoder().decode(original));
  expect(transcript).not.toContain(new TextDecoder().decode(replacement));
}, 240_000);

test.serial("live.storage-persist-overwrites-current-without-guest-aws-credentials", async () => {
  if (inventory()) return;
  const name = unique("persisted");
  createdFiles.add(name);
  const marker = `persist-${randomUUID()}`;
  const session = await createSession();
  createdSessions.add(session.id);
  await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      text: `Use write_file to write exactly ${JSON.stringify(marker)} to /workspace/outputs/result.txt, then invoke storage.persist (provider function storage_persist) for that path with logical name ${name} and media type text/plain. Do not merely describe the calls.`,
    },
  });
  await waitForAssistantText(session.id);
  const persisted = await waitForReady(name);
  expect(persisted.value.content.sha256).toBe(digest(new TextEncoder().encode(marker)));
}, 240_000);

test("live.registry-list", async () => {
  if (inventory()) return;
  const body = await jsonRequest<{ items?: unknown }>("/api/files?limit=1");
  expect(Array.isArray(body.items)).toBe(true);
}, 30_000);

test.serial("live.session-message-stream-structured-output", async () => {
  if (inventory()) return;
  const marker = `structured-${randomUUID()}`;
  const session = await createSession(undefined, { sandbox: { enabled: false } });
  createdSessions.add(session.id);
  expect(session.sandboxStatus).toBe("disabled");
  expect(session.resolvedConfig.sandboxEnabled).toBe(false);

  const preview = readNdjsonUntil<MessageStreamFrame>(
    `/api/sessions/${encodeURIComponent(session.id)}/messages/stream`,
    (frame) => frame.kind === "preview" && typeof frame.text === "string",
  );
  const reconciled = readNdjsonUntil<MessageStreamFrame>(
    `/api/sessions/${encodeURIComponent(session.id)}/messages/stream`,
    (frame) => frame.kind === "reconcile" && frame.message?.role === "assistant",
  );
  await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      text: `Return JSON whose marker property is exactly ${JSON.stringify(marker)}.`,
      responseFormat: {
        kind: "json_schema",
        schema: {
          type: "object",
          additionalProperties: false,
          required: ["marker"],
          properties: { marker: { const: marker } },
        },
      },
    },
  });
  expect((await preview).messageId).toBeTruthy();
  const frame = await reconciled;
  const text = messageText(frame.message);
  expect(JSON.parse(text)).toEqual({ marker });
}, 240_000);

test.serial("live.sandbox-disabled-is-a-structured-tool-error", async () => {
  if (inventory()) return;
  const session = await createSession(undefined, { sandbox: { enabled: false } });
  createdSessions.add(session.id);
  await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      text: "Invoke read_file for /workspace/does-not-exist. Inspect the tool result and reply with only its exact machine-readable error code.",
    },
  });
  expect((await waitForAssistantText(session.id)).toLowerCase()).toContain("sandbox_disabled");
}, 240_000);

test.serial("live.telemetry-stream-replay-and-compressed-download", async () => {
  if (inventory()) return;
  const marker = `telemetry-${randomUUID()}`;
  const session = await createSession();
  createdSessions.add(session.id);
  await waitForSandbox(session.id);

  const liveFrame = readNdjsonUntil<TelemetryFrame>(
    `/api/sessions/${encodeURIComponent(session.id)}/telemetry/stream`,
    ({ kind }) => !["heartbeat", "gap"].includes(kind),
  );
  await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      text: `Use bash to run printf with ${JSON.stringify(marker)}, then reply with only that output.`,
    },
  });
  expect(await waitForAssistantText(session.id)).toContain(marker);
  expect((await liveFrame).sequence).toMatch(/^(0|[1-9][0-9]*)$/);

  const replay = await readNdjsonAll<TelemetryFrame>(
    `/api/sessions/${encodeURIComponent(session.id)}/telemetry/replay?after=0&limit=100`,
  );
  expect(replay.some(({ kind }) => !["heartbeat", "gap"].includes(kind))).toBe(true);
  const grant = await jsonRequest<TelemetryDownloadGrant>(
    `/api/sessions/${encodeURIComponent(session.id)}/telemetry/downloads`,
    { method: "POST", idempotencyKey: randomUUID(), body: {} },
  );
  const compressed = new Uint8Array(await (await fetch(grant.url, {
    signal: AbortSignal.timeout(60_000),
  })).arrayBuffer());
  expect(compressed.byteLength).toBe(Number(grant.sizeBytes));
  expect(digest(compressed)).toBe(grant.sha256);
  expect(grant.mediaType).toContain("ndjson");
  expect(grant.mediaType).toContain("zstd");
}, 300_000);

test.serial("live.native-subagent-create-and-event-driven-wait", async () => {
  if (inventory()) return;
  const session = await createSession(undefined, { sandbox: { enabled: false } });
  createdSessions.add(session.id);
  await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      text: "Invoke create_subagent with a child prompt that returns CHILD_FINISHED. Then invoke wait_subagents for that child in all mode. After the durable wait reports terminal, reply with only SUBAGENT_DONE.",
    },
  });
  expect(await waitForAssistantText(session.id)).toContain("SUBAGENT_DONE");
}, 300_000);

test.serial("live.remote-mcp-is-qualified-and-invoked-through-tool-mux", async () => {
  if (inventory()) return;
  const fixture = mcpFixture("REMOTE");
  const session = await createSession(undefined, {
    mcpServers: [{
      name: fixture.server,
      transport: {
        kind: "remote_http",
        url: required("AEX_LIVE_REMOTE_MCP_URL"),
        ...(optionalJson<Record<string, string>>("AEX_LIVE_REMOTE_MCP_HEADERS") === undefined
          ? {}
          : { headers: optionalJson<Record<string, string>>("AEX_LIVE_REMOTE_MCP_HEADERS") }),
      },
    }],
  });
  createdSessions.add(session.id);
  await invokeMcpFixture(session.id, fixture);
}, 300_000);

test.serial("live.sandbox-mcp-is-qualified-and-invoked-through-the-hand", async () => {
  if (inventory()) return;
  const fixture = mcpFixture("SANDBOX");
  const transport = requiredJson<Record<string, unknown>>("AEX_LIVE_SANDBOX_MCP_TRANSPORT");
  const session = await createSession(undefined, {
    mcpServers: [{ name: fixture.server, transport: { kind: "sandbox_process", ...transport } }],
  });
  createdSessions.add(session.id);
  await waitForSandbox(session.id);
  await invokeMcpFixture(session.id, fixture);
}, 360_000);

test.serial("live.seven-official-provider-families", async () => {
  if (inventory()) return;
  const cases = requiredJson<ReadonlyArray<ProviderCase>>("AEX_LIVE_PROVIDER_CASES");
  expect(new Set(cases.map(({ provider }) => provider))).toEqual(new Set([
    "openai", "anthropic", "deepseek", "xai", "meta", "moonshotai", "alibaba",
  ]));
  for (const providerCase of cases) {
    const session = await createSession(undefined, {
      provider: providerCase.provider,
      model: providerCase.model,
      providerApiKey: providerCase.apiKey,
      sandbox: { enabled: false },
    });
    createdSessions.add(session.id);
    await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
      method: "POST",
      idempotencyKey: randomUUID(),
      body: { text: `Reply with only PROVIDER_${providerCase.provider.toUpperCase()}.` },
    });
    expect(await waitForAssistantText(session.id)).toContain(`PROVIDER_${providerCase.provider.toUpperCase()}`);
  }
}, 600_000);

test.serial("live.essential-billing-read-card-and-topup-surfaces", async () => {
  if (inventory()) return;
  const [balance, paymentMethods, usage, transactions] = await Promise.all([
    jsonRequest<BillingBalance>("/api/billing/balance"),
    jsonRequest<{ items: unknown[] }>("/api/billing/payment-methods"),
    jsonRequest<{ items: unknown[]; coverage: { hasGap: boolean } }>("/api/billing/usage?limit=10"),
    jsonRequest<{ items: unknown[] }>("/api/billing/transactions?limit=10"),
  ]);
  expect(balance.currency).toBe("USD");
  expect(balance.availableCents).toMatch(/^(0|[1-9][0-9]*)$/);
  expect(Array.isArray(paymentMethods.items)).toBe(true);
  expect(Array.isArray(usage.items)).toBe(true);
  expect(typeof usage.coverage.hasGap).toBe("boolean");
  expect(Array.isArray(transactions.items)).toBe(true);

  const card = await jsonRequest<HostedSession>("/api/billing/payment-method-sessions", {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: { consent: true },
  });
  const topup = await jsonRequest<HostedSession>("/api/billing/top-up-checkouts", {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: { amountCents: required("AEX_LIVE_TOPUP_AMOUNT_CENTS") },
  });
  expect(new URL(card.url).protocol).toBe("https:");
  expect(new URL(topup.url).protocol).toBe("https:");
}, 180_000);

test.serial("live.session-termination-retains-history-and-rejects-new-work", async () => {
  if (inventory()) return;
  const marker = `terminal-${randomUUID()}`;
  const session = await createSession(undefined, { sandbox: { enabled: false } });
  createdSessions.add(session.id);
  await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: { text: `Reply with only ${marker}.` },
  });
  expect(await waitForAssistantText(session.id)).toContain(marker);

  const receipt = await jsonRequest<SessionCommandReceipt>(
    `/api/sessions/${encodeURIComponent(session.id)}/terminations`,
    { method: "POST", operationId: newId("operation"), body: {} },
  );
  expect(receipt.sessionId).toBe(session.id);
  const terminal = await waitForSessionStatus(session.id, "terminated");
  expect(terminal.terminatedAt).toBeDefined();
  expect(await waitForAssistantText(session.id)).toContain(marker);
  await request(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: { text: "This must not be admitted after termination." },
  }, 409);
}, 300_000);

test.serial("live.long-context-rolls-forward-without-a-semantic-turn-cap", async () => {
  if (inventory()) return;
  const turns = Number(process.env.AEX_LIVE_LONG_CONTEXT_TURNS ?? "24");
  if (!Number.isSafeInteger(turns) || turns < 12 || turns > 64) {
    throw new Error("AEX_LIVE_LONG_CONTEXT_TURNS must be an integer in 12..=64");
  }
  const first = `LONG_FIRST_${randomUUID()}`;
  const last = `LONG_LAST_${randomUUID()}`;
  const session = await createSession(undefined, { sandbox: { enabled: false } });
  createdSessions.add(session.id);
  for (let index = 0; index < turns; index += 1) {
    const marker = index === 0 ? first : index === turns - 1 ? last : `LONG_${index}_${randomUUID()}`;
    await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
      method: "POST",
      idempotencyKey: randomUUID(),
      body: { text: `Remember ${marker} for this conversation and reply with only ACK_${index}.` },
    });
    const replies = await waitForAssistantCount(session.id, index + 1);
    expect(replies.at(-1)).toContain(`ACK_${index}`);
  }
  await jsonRequest(`/api/sessions/${encodeURIComponent(session.id)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: { text: "Reply with the exact LONG_FIRST and LONG_LAST markers you were asked to remember, and nothing else." },
  });
  const final = (await waitForAssistantCount(session.id, turns + 1)).at(-1) ?? "";
  expect(final).toContain(first);
  expect(final).toContain(last);
}, 900_000);

afterAll(async () => {
  if (inventory()) return;
  const residue: Array<{ kind: string; id: string }> = [];
  for (const session of createdSessions) {
    try {
      await jsonRequest(`/api/sessions/${encodeURIComponent(session)}/deletions`, {
        method: "POST",
        operationId: newId("operation"),
        body: {},
      });
      await waitForSessionDeleted(session);
    } catch {
      residue.push({ kind: "session", id: session });
    }
  }
  for (const name of createdFiles) {
    try {
      await request(`/api/files/${encodeURIComponent(name)}`, { method: "DELETE" }, 204);
    } catch {
      residue.push({ kind: "file", id: name });
    }
  }
  const cleanupLedgerDigest = `sha256:${createHash("sha256").update(JSON.stringify(residue)).digest("hex")}`;
  writeFileSync(required("AEX_RELEASE_EVIDENCE_HYGIENE_PATH"), `${JSON.stringify({
    schema: "aex.release-evidence-hygiene.v1",
    suite: "user",
    releaseId: required("RELEASE_ID"),
    workflowRunId: required("GITHUB_RUN_ID"),
    budgetMicroUsd: 0,
    spentMicroUsd: 0,
    cleanupLedgerDigest,
    provisioned: [],
    residue,
    secretCanaryDigest: required("SECRET_CANARY_DIGEST"),
    secretCanaryObserved: false,
  })}\n`);
});

interface WorkspaceFile {
  readonly name: string;
  readonly state: "pending" | "ready" | "failed";
  readonly value?: { readonly content: { readonly sha256: string; readonly sizeBytes: string } };
}

interface ReadyWorkspaceFile extends WorkspaceFile {
  readonly state: "ready";
  readonly value: { readonly content: { readonly sha256: string; readonly sizeBytes: string } };
}

interface DownloadGrant {
  readonly url: string;
  readonly sha256: string;
}

interface UploadAdmission {
  readonly upload: {
    readonly id: string;
    readonly partCount: number;
    readonly partSizeBytes: string;
  };
  readonly grants: ReadonlyArray<{
    readonly partNumber: number;
    readonly url: string;
    readonly headers: ReadonlyArray<{ readonly name: string; readonly value: string }>;
  }>;
}

interface SessionRecord {
  readonly id: string;
  readonly sandboxStatus: string;
  readonly status: string;
  readonly terminatedAt?: string;
  readonly resolvedConfig: { readonly sandboxEnabled: boolean };
}

interface SessionCommandReceipt {
  readonly sessionId: string;
  readonly operationId: string;
  readonly acceptedAt: string;
}

interface MessagePart {
  readonly type: string;
  readonly text?: string;
}

interface SessionMessage {
  readonly role: string;
  readonly content: ReadonlyArray<MessagePart>;
}

interface MessageStreamFrame {
  readonly kind: string;
  readonly messageId?: string;
  readonly text?: string;
  readonly message?: SessionMessage;
}

interface TelemetryFrame {
  readonly sequence: string;
  readonly kind: string;
}

interface TelemetryDownloadGrant {
  readonly url: string;
  readonly sha256: string;
  readonly sizeBytes: string;
  readonly mediaType: string;
}

interface ProviderCase {
  readonly provider: "openai" | "anthropic" | "deepseek" | "xai" | "meta" | "moonshotai" | "alibaba";
  readonly model: string;
  readonly apiKey: string;
}

interface BillingBalance {
  readonly currency: string;
  readonly availableCents: string;
}

interface HostedSession {
  readonly url: string;
}

interface SessionOptions {
  readonly provider?: ProviderCase["provider"];
  readonly model?: string;
  readonly providerApiKey?: string;
  readonly sandbox?: Record<string, unknown>;
  readonly mcpServers?: ReadonlyArray<Record<string, unknown>>;
}

interface McpFixture {
  readonly server: string;
  readonly tool: string;
  readonly arguments: unknown;
  readonly expected: string;
}

async function putInline(
  name: string,
  bytes: Uint8Array,
  mediaType: string,
): Promise<ReadyWorkspaceFile> {
  return jsonRequest(`/api/files/${encodeURIComponent(name)}`, {
    method: "PUT",
    idempotencyKey: randomUUID(),
    body: {
      content: { type: "inline", encoding: "base64", data: Buffer.from(bytes).toString("base64"), sha256: digest(bytes) },
      mediaType,
      mode: "0644",
    },
  });
}

async function createSession(fileName?: string, options: SessionOptions = {}): Promise<SessionRecord> {
  return jsonRequest("/api/sessions", {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      provider: options.provider ?? required("AEX_LIVE_PROVIDER"),
      model: options.model ?? required("AEX_LIVE_MODEL"),
      providerApiKey: options.providerApiKey ?? required("AEX_LIVE_PROVIDER_API_KEY"),
      ...(options.sandbox === undefined ? {} : { sandbox: options.sandbox }),
      ...(options.mcpServers === undefined ? {} : { mcpServers: options.mcpServers }),
      ...(fileName ? {
        registered: {
          mounts: [{ name: fileName, path: "/workspace/inputs/frozen.txt" }],
        },
      } : {}),
    },
  });
}

async function waitForReady(name: string): Promise<ReadyWorkspaceFile> {
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    const current = await jsonRequest<WorkspaceFile>(`/api/files/${encodeURIComponent(name)}`);
    if (current.state === "ready" && current.value !== undefined) return current as ReadyWorkspaceFile;
    if (current.state === "failed") throw new Error(`file ${name} import failed`);
    await Bun.sleep(500);
  }
  throw new Error(`file ${name} did not become ready`);
}

async function waitForAssistantText(session: string): Promise<string> {
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const messages = await listAllMessages(session);
    const assistant = messages.filter(({ role }) => role === "assistant").at(-1);
    if (assistant) return assistant.content.map(({ text }) => text ?? "").join("");
    await Bun.sleep(1_000);
  }
  throw new Error(`session ${session} produced no assistant message`);
}

async function waitForAssistantCount(session: string, count: number): Promise<ReadonlyArray<string>> {
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const assistant = (await listAllMessages(session))
      .filter(({ role }) => role === "assistant")
      .map(({ content }) => content.map(({ text }) => text ?? "").join(""));
    if (assistant.length >= count) return assistant;
    await Bun.sleep(1_000);
  }
  throw new Error(`session ${session} produced fewer than ${count} assistant messages`);
}

async function listAllMessages(session: string): Promise<ReadonlyArray<SessionMessage>> {
  const items: SessionMessage[] = [];
  let cursor: string | undefined;
  do {
    const query = new URLSearchParams({ limit: "100" });
    if (cursor !== undefined) query.set("cursor", cursor);
    const page = await jsonRequest<{
      items: ReadonlyArray<SessionMessage>;
      nextCursor?: string;
    }>(`/api/sessions/${encodeURIComponent(session)}/messages?${query}`);
    items.push(...page.items);
    cursor = page.nextCursor;
  } while (cursor !== undefined);
  return items;
}

async function waitForSandbox(session: string): Promise<SessionRecord> {
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const current = await jsonRequest<SessionRecord>(`/api/sessions/${encodeURIComponent(session)}`);
    if (["ready", "suspended"].includes(current.sandboxStatus)) return current;
    if (["disabled", "lost"].includes(current.sandboxStatus)) {
      throw new Error(`session ${session} sandbox reached ${current.sandboxStatus}`);
    }
    await Bun.sleep(500);
  }
  throw new Error(`session ${session} sandbox was not prepared`);
}

async function waitForSessionStatus(session: string, status: string): Promise<SessionRecord> {
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const current = await jsonRequest<SessionRecord>(`/api/sessions/${encodeURIComponent(session)}`);
    if (current.status === status) return current;
    await Bun.sleep(500);
  }
  throw new Error(`session ${session} did not reach ${status}`);
}

async function waitForSessionDeleted(session: string): Promise<void> {
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const response = await fetch(
      new URL(`/api/sessions/${encodeURIComponent(session)}`, required("AEX_API_URL")),
      {
        headers: { authorization: `Bearer ${required("AEX_API_KEY")}` },
        signal: AbortSignal.timeout(30_000),
      },
    );
    if (response.status === 404 || response.status === 410) return;
    if (!response.ok) {
      throw new Error(`GET deleted session ${session}: ${response.status}: ${await response.text()}`);
    }
    await Bun.sleep(500);
  }
  throw new Error(`session ${session} deletion did not become visible`);
}

async function readNdjsonUntil<T>(path: string, predicate: (value: T) => boolean): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 240_000);
  try {
    const response = await streamRequest(path, controller.signal);
    for await (const value of ndjson<T>(response)) {
      if (predicate(value)) {
        controller.abort();
        return value;
      }
    }
  } finally {
    clearTimeout(timeout);
  }
  throw new Error(`${path} ended before the expected frame`);
}

async function readNdjsonAll<T>(path: string): Promise<ReadonlyArray<T>> {
  const response = await streamRequest(path, AbortSignal.timeout(90_000));
  const values: T[] = [];
  for await (const value of ndjson<T>(response)) values.push(value);
  return values;
}

async function streamRequest(path: string, signal: AbortSignal): Promise<Response> {
  const response = await fetch(new URL(path, required("AEX_API_URL")), {
    headers: {
      accept: "application/x-ndjson",
      authorization: `Bearer ${required("AEX_API_KEY")}`,
    },
    signal,
  });
  if (!response.ok) throw new Error(`GET ${path}: ${response.status}: ${await response.text()}`);
  if (!response.headers.get("content-type")?.includes("application/x-ndjson")) {
    throw new Error(`GET ${path}: expected application/x-ndjson`);
  }
  return response;
}

async function* ndjson<T>(response: Response): AsyncGenerator<T> {
  if (response.body === null) throw new Error("NDJSON response has no body");
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  while (true) {
    const { done, value } = await reader.read();
    pending += decoder.decode(value, { stream: !done });
    let newline = pending.indexOf("\n");
    while (newline >= 0) {
      const line = pending.slice(0, newline).trim();
      pending = pending.slice(newline + 1);
      if (line.length > 0) yield JSON.parse(line) as T;
      newline = pending.indexOf("\n");
    }
    if (done) break;
  }
  if (pending.trim().length > 0) yield JSON.parse(pending) as T;
}

function messageText(message: SessionMessage | undefined): string {
  if (message === undefined) throw new Error("committed frame omitted its message");
  return message.content.map(({ text }) => text ?? "").join("");
}

function mcpFixture(kind: "REMOTE" | "SANDBOX"): McpFixture {
  return {
    server: required(`AEX_LIVE_${kind}_MCP_SERVER`),
    tool: required(`AEX_LIVE_${kind}_MCP_TOOL`),
    arguments: requiredJson(`AEX_LIVE_${kind}_MCP_ARGUMENTS`),
    expected: required(`AEX_LIVE_${kind}_MCP_EXPECTED`),
  };
}

async function invokeMcpFixture(session: string, fixture: McpFixture): Promise<void> {
  await jsonRequest(`/api/sessions/${encodeURIComponent(session)}/messages`, {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      text: `Invoke the MCP tool ${fixture.tool} with exactly these JSON arguments: ${JSON.stringify(fixture.arguments)}. Then reply with its result.`,
    },
  });
  expect(await waitForAssistantText(session)).toContain(fixture.expected);
}

interface RequestOptions {
  readonly method?: string;
  readonly body?: unknown;
  readonly idempotencyKey?: string;
  readonly operationId?: string;
}

async function jsonRequest<T = unknown>(path: string, options: RequestOptions = {}): Promise<T> {
  const response = await request(path, options);
  return response.json() as Promise<T>;
}

async function request(path: string, options: RequestOptions = {}, expected?: number): Promise<Response> {
  const headers = new Headers({ authorization: `Bearer ${required("AEX_API_KEY")}` });
  if (options.body !== undefined) headers.set("content-type", "application/json");
  if (options.idempotencyKey) headers.set("idempotency-key", options.idempotencyKey);
  if (options.operationId) headers.set("aex-operation-id", options.operationId);
  const response = await fetch(new URL(path, required("AEX_API_URL")), {
    ...(options.method === undefined ? {} : { method: options.method }),
    headers,
    ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) }),
    signal: AbortSignal.timeout(90_000),
  });
  if (expected === undefined ? !response.ok : response.status !== expected) {
    throw new Error(`${options.method ?? "GET"} ${path}: expected ${expected ?? "2xx"}, got ${response.status}: ${await response.text()}`);
  }
  return response;
}

function digest(bytes: Uint8Array): string {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function unique(prefix: string): string {
  return `${prefix}-${randomUUID()}`;
}

function inventory(): boolean {
  return process.env.AEX_RELEASE_EVIDENCE_MODE === "inventory";
}

function required(name: string): string {
  const value = process.env[name];
  if (typeof value !== "string" || value.length === 0) throw new Error(`${name} is required`);
  return value;
}

function requiredJson<T = unknown>(name: string): T {
  return JSON.parse(required(name)) as T;
}

function optionalJson<T>(name: string): T | undefined {
  const value = process.env[name];
  return value === undefined || value.length === 0 ? undefined : JSON.parse(value) as T;
}
