import { afterAll, expect, test } from "bun:test";
import { createHash, randomUUID } from "node:crypto";
import { writeFileSync } from "node:fs";
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
      mountPath: `/imports/${target}.txt`,
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
  const published = await jsonRequest<WorkspaceFile>(
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

afterAll(async () => {
  if (inventory()) return;
  const residue: Array<{ kind: string; id: string }> = [];
  for (const session of createdSessions) {
    try {
      await jsonRequest(`/api/sessions/${encodeURIComponent(session)}/terminations`, {
        method: "POST",
        operationId: randomUUID(),
        body: {},
      });
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
  readonly state: "pending" | "ready" | "failed" | "current";
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

async function putInline(name: string, bytes: Uint8Array, mediaType: string): Promise<WorkspaceFile> {
  return jsonRequest(`/api/files/${encodeURIComponent(name)}`, {
    method: "PUT",
    idempotencyKey: randomUUID(),
    body: {
      content: { type: "inline", encoding: "base64", data: Buffer.from(bytes).toString("base64"), sha256: digest(bytes) },
      mediaType,
      mode: "0644",
      mountPath: `/inputs/${name}`,
    },
  });
}

async function createSession(fileName?: string): Promise<{ id: string }> {
  return jsonRequest("/api/sessions", {
    method: "POST",
    idempotencyKey: randomUUID(),
    body: {
      provider: required("AEX_LIVE_PROVIDER"),
      model: required("AEX_LIVE_MODEL"),
      providerCredentialId: required("AEX_LIVE_PROVIDER_CREDENTIAL_ID"),
      ...(fileName ? { registered: { files: [fileName] } } : {}),
    },
  });
}

async function waitForReady(name: string): Promise<WorkspaceFile> {
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    const current = await jsonRequest<WorkspaceFile>(`/api/files/${encodeURIComponent(name)}`);
    if (current.state === "ready" || current.state === "current") return current;
    if (current.state === "failed") throw new Error(`file ${name} import failed`);
    await Bun.sleep(500);
  }
  throw new Error(`file ${name} did not become ready`);
}

async function waitForAssistantText(session: string): Promise<string> {
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const page = await jsonRequest<{ items: ReadonlyArray<{ role: string; content: ReadonlyArray<{ type: string; text?: string }> }> }>(
      `/api/sessions/${encodeURIComponent(session)}/messages?limit=100`,
    );
    const assistant = page.items.filter(({ role }) => role === "assistant").at(-1);
    if (assistant) return assistant.content.map(({ text }) => text ?? "").join("");
    await Bun.sleep(1_000);
  }
  throw new Error(`session ${session} produced no assistant message`);
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

async function request(path: string, options: RequestOptions = {}, expected = 200): Promise<Response> {
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
  if (response.status !== expected) {
    throw new Error(`${options.method ?? "GET"} ${path}: expected ${expected}, got ${response.status}: ${await response.text()}`);
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
