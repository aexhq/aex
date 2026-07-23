import fc from "fast-check";
import { describe, expect, it, setDefaultTimeout } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import {
  BUILTIN_TOOL_NAMES,
  SUPPORTED_MODELS,
  PROVIDERS,
  RUNTIME_SIZES,
  RUNTIME_KINDS,
  providersForModel,
  type BuiltinToolName,
  type JsonValue,
  type OutputMode,
  type ModelName,
  type ProviderName,
  type RuntimeSize,
  type RuntimeKind,
  type SessionRuntime
} from "@aexhq/contracts";
import { parseSessionSubmissionRequest } from "@aexhq/contracts/internal";
import {
  Aex,
  Models,
  SessionConfigValidationError,
  Secret,
  type SessionCreateOptions
} from "../../src/index.js";

type Mutable<T> = { -readonly [K in keyof T]: T[K] };
type CreateSurface = "sessions.create";

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

interface CaptureHarness {
  readonly client: Aex;
  readonly calls: CapturedRequest[];
}

interface ProviderChoice {
  readonly model: ModelName;
  readonly provider?: ProviderName;
  readonly resolvedProvider: ProviderName;
}

interface ValidCase {
  readonly surface: CreateSurface;
  readonly options: SessionCreateOptions;
  readonly resolvedProvider: ProviderName;
}

const createSurface = fc.constant<CreateSurface>("sessions.create");
const modelName = fc.constantFrom<ModelName>(...(SUPPORTED_MODELS as readonly ModelName[]));
const runtimeSize = fc.constantFrom<RuntimeSize>(...(RUNTIME_SIZES as readonly RuntimeSize[]));
const runtimeKind = fc.constantFrom<RuntimeKind>(...(RUNTIME_KINDS as readonly RuntimeKind[]));
// The grouped runtime selector: at least one of { kind, size } present.
const runtimeArb: fc.Arbitrary<SessionRuntime> = fc
  .record({ kind: fc.option(runtimeKind, { nil: undefined }), size: fc.option(runtimeSize, { nil: undefined }) })
  .filter((r) => r.kind !== undefined || r.size !== undefined)
  .map((r): SessionRuntime => ({
    ...(r.kind !== undefined ? { kind: r.kind } : {}),
    ...(r.size !== undefined ? { size: r.size } : {})
  }));
const outputMode = fc.constant<OutputMode>("buffered");
const safeToken = fc.stringMatching(/^[A-Za-z0-9_-]{1,24}$/);
const shortText = fc.stringMatching(/^[A-Za-z0-9 .,:/_-]{1,64}$/);
const envValue = fc.stringMatching(/^[A-Za-z0-9 .,:/_-]{0,64}$/);
const idempotencyKey = fc.stringMatching(/^[A-Za-z][A-Za-z0-9_-]{0,39}$/);
const duration = fc.constantFrom("1m", "90s", "5m", "30m", "1h", "3600s", "6h");
const webhook = fc
  .tuple(fc.integer({ min: 1, max: 999_999 }), fc.integer({ min: 1, max: 999_999 }))
  .map(([host, path]) => ({ url: `https://hooks-${host}.example.test/aex/${path}` }));
const pathSegment = fc.stringMatching(/^[a-z][a-z0-9_-]{0,10}$/);
const outputPath = fc
  .array(pathSegment, { minLength: 1, maxLength: 4 })
  .map((segments) => `/${segments.join("/")}`);
const builtinToolNames = [...BUILTIN_TOOL_NAMES] as BuiltinToolName[];
const builtinTools = fc.subarray(builtinToolNames, {
  minLength: 0,
  maxLength: 5
});
const nonEmptyBuiltinTools = fc.subarray(builtinToolNames, {
  minLength: 1,
  maxLength: 5
});

const providerChoice: fc.Arbitrary<ProviderChoice> = modelName.chain((model) => {
  const providers = providersForModel(model);
  const defaultProvider = providers[0];
  if (defaultProvider === undefined) {
    throw new Error(`test invariant failed: model ${model} has no provider`);
  }
  return fc.oneof(
    fc.constant({ model, resolvedProvider: defaultProvider }),
    fc.constantFrom(...providers).map((provider) => ({
      model,
      provider,
      resolvedProvider: provider
    }))
  );
});

const jsonScalar: fc.Arbitrary<JsonValue> = fc.oneof(
  shortText,
  fc.integer({ min: -10_000, max: 10_000 }),
  fc.boolean(),
  fc.constant(null)
);

function numberedRecord<T>(
  prefix: string,
  value: fc.Arbitrary<T>,
  options: { readonly minLength?: number; readonly maxLength: number }
): fc.Arbitrary<Record<string, T>> {
  return fc
    .uniqueArray(fc.integer({ min: 1, max: 999_999 }), {
      minLength: options.minLength ?? 0,
      maxLength: options.maxLength
    })
    .chain((keys) =>
      fc.array(value, { minLength: keys.length, maxLength: keys.length }).map((values) => {
        const out: Record<string, T> = {};
        keys.forEach((key, index) => {
          const entry = values[index];
          if (entry !== undefined) out[`${prefix}${key}`] = entry;
        });
        return out;
      })
    );
}

const jsonValue: fc.Arbitrary<JsonValue> = fc.oneof(
  jsonScalar,
  fc.array(jsonScalar, { maxLength: 3 }),
  numberedRecord("nested_", jsonScalar, { maxLength: 3 })
);
const metadata = numberedRecord("meta_", jsonValue, { maxLength: 5 });
const nonEmptyMetadata = numberedRecord("meta_", jsonValue, { minLength: 1, maxLength: 5 });
const variables = numberedRecord("VAR_", envValue, { maxLength: 5 });
const nonEmptyVariables = numberedRecord("VAR_", envValue, { minLength: 1, maxLength: 5 });
const fakeSecretValue = safeToken.map((value) => `fake-${value}`);
const secretRef = safeToken.map((value) => Secret.ref(`ref-${value}`));
const secretValue = fakeSecretValue.map((value) => Secret.value(value));
const secret = fc.oneof(secretRef, secretValue);
const environmentSecrets = numberedRecord("SECRET_", secret, { maxLength: 4 });
const nonEmptyEnvironmentSecrets = numberedRecord("SECRET_", secret, { minLength: 1, maxLength: 4 });

const fileCaptureDirs = (minLength: number) =>
  fc.uniqueArray(outputPath, { minLength, maxLength: 4 });
const positiveInteger = fc.integer({ min: 1, max: 1_000_000 });
const fileCaptureTimeoutMs = fc.integer({ min: 1, max: 6 * 60 * 60 * 1000 });

const richFileCapture = fc
  .record({
    allowedDirs: fileCaptureDirs(1),
    deniedDirs: fileCaptureDirs(1),
    captureTimeoutMs: fileCaptureTimeoutMs,
    maxFileBytes: positiveInteger,
    maxTotalBytes: positiveInteger,
    maxFiles: fc.integer({ min: 1, max: 10_000 })
  })
  .map((parts) => buildFileCapture(parts));

const sparseFileCapture = fc
  .record(
    {
      allowedDirs: fc.option(fileCaptureDirs(0), { nil: undefined }),
      deniedDirs: fc.option(fileCaptureDirs(0), { nil: undefined }),
      captureTimeoutMs: fc.option(fileCaptureTimeoutMs, { nil: undefined }),
      maxFileBytes: fc.option(positiveInteger, { nil: undefined }),
      maxTotalBytes: fc.option(positiveInteger, { nil: undefined }),
      maxFiles: fc.option(fc.integer({ min: 1, max: 10_000 }), { nil: undefined })
    },
    { requiredKeys: [] }
  )
  .map((parts) => buildFileCapture(parts));

const spendLimit = fc.integer({ min: 1, max: 100_000 }).map((cents) => cents / 100);
const richOverrides = fc
  .record({
    idleTtl: duration,
    timeout: duration,
    maxSpendUsd: spendLimit
  })
  .map((parts) => buildOverrides(parts));
const sparseOverrides = fc
  .record(
    {
      idleTtl: fc.option(duration, { nil: undefined }),
      timeout: fc.option(duration, { nil: undefined }),
      maxSpendUsd: fc.option(spendLimit, { nil: undefined })
    },
    { requiredKeys: [] }
  )
  .map((parts) => buildOverrides(parts));

const richEnvironment = fc
  .record({
    variables: nonEmptyVariables,
    secrets: nonEmptyEnvironmentSecrets
  })
  .map((parts) => buildEnvironment(parts));
const sparseEnvironment = fc
  .record(
    {
      variables: fc.option(variables, { nil: undefined }),
      secrets: fc.option(environmentSecrets, { nil: undefined })
    },
    { requiredKeys: [] }
  )
  .map((parts) => buildEnvironment(parts));

const richValidCase = providerChoice.chain((choice) =>
  apiKeysFor(choice.resolvedProvider).chain((apiKeys) =>
    fc
      .record({
        surface: createSurface,
        system: shortText,
        metadata: nonEmptyMetadata,
        environment: richEnvironment,
        fileCapture: richFileCapture,
        overrides: richOverrides,
        runtime: runtimeArb,
        outputMode,
        builtinTools: fc.oneof(fc.constant("default" as const), fc.constant("none" as const), nonEmptyBuiltinTools),
        webhook,
        idempotencyKey,
      })
      .map((parts): ValidCase => ({
        surface: parts.surface,
        resolvedProvider: choice.resolvedProvider,
        options: buildSessionOptions(choice, {
          apiKeys,
          system: parts.system,
          metadata: parts.metadata,
          environment: parts.environment,
          fileCapture: parts.fileCapture,
          overrides: parts.overrides,
          runtime: parts.runtime,
          outputMode: parts.outputMode,
          builtinTools: parts.builtinTools,
          webhook: parts.webhook,
          idempotencyKey: parts.idempotencyKey,
        })
      }))
  )
);

const sparseValidCase = providerChoice.chain((choice) =>
  apiKeysFor(choice.resolvedProvider).chain((apiKeys) =>
    fc
      .record(
        {
          surface: createSurface,
          system: fc.option(shortText, { nil: undefined }),
          metadata: fc.option(metadata, { nil: undefined }),
          environment: fc.option(sparseEnvironment, { nil: undefined }),
          fileCapture: fc.option(sparseFileCapture, { nil: undefined }),
          overrides: fc.option(sparseOverrides, { nil: undefined }),
          runtime: fc.option(runtimeArb, { nil: undefined }),
          outputMode: fc.option(outputMode, { nil: undefined }),
          builtinTools: fc.option(
            fc.oneof(fc.constant("default" as const), fc.constant("none" as const), builtinTools),
            { nil: undefined }
          ),
          webhook: fc.option(webhook, { nil: undefined }),
          idempotencyKey: fc.option(idempotencyKey, { nil: undefined }),
        },
        { requiredKeys: ["surface"] }
      )
      .map((parts): ValidCase => ({
        surface: parts.surface,
        resolvedProvider: choice.resolvedProvider,
        options: buildSessionOptions(choice, {
          apiKeys,
          system: parts.system,
          metadata: parts.metadata,
          environment: parts.environment,
          fileCapture: parts.fileCapture,
          overrides: parts.overrides,
          runtime: parts.runtime,
          outputMode: parts.outputMode,
          builtinTools: parts.builtinTools,
          webhook: parts.webhook,
          idempotencyKey: parts.idempotencyKey,
        })
      }))
  )
);

const legacyProxyField = "proxy" + "Endpoints";
const legacyFields = [
  "prompt",
  "input",
  "runtimeSize",
  "secretEnv",
  "secrets",
  legacyProxyField,
  "postHook",
  "limits",
  "timeout",
  "instructions",
  "idleSuspendAfter",
  "idleTtl",
  "retention",
  "parentSessionId",
  "signal"
] as const;
const legacyValue = fc.oneof(
  fc.constant(undefined),
  fc.constant(null),
  shortText,
  fc.boolean(),
  fc.integer({ min: -10, max: 10 }),
  fc.array(shortText, { maxLength: 3 }),
  fc.dictionary(fc.stringMatching(/^[a-z][a-z0-9_]{0,8}$/), shortText, { maxKeys: 3 })
);

const providerMismatch = modelName.chain((model) => {
  const supported = providersForModel(model);
  const unsupported = PROVIDERS.filter((provider) => !supported.includes(provider));
  return fc.constantFrom(...unsupported).map((provider) =>
    buildSessionOptions(
      { model, provider, resolvedProvider: provider },
      { apiKeys: { [provider]: `fake-${provider}-key` } }
    )
  );
});

const missingProviderKey = providerChoice.chain((choice) => {
  const otherProviders = PROVIDERS.filter((provider) => provider !== choice.resolvedProvider);
  return fc
    .oneof(
      fc.constant(undefined),
      fc.constant({}),
      fc.constantFrom(...otherProviders).map((provider) => ({ [provider]: `fake-${provider}-key` }))
    )
    .map((apiKeys) => buildSessionOptions(choice, apiKeys === undefined ? {} : { apiKeys }));
});

function captureClient(): CaptureHarness {
  const calls: CapturedRequest[] = [];
  const fetchImpl: FetchLike = async (input, init) => {
    const url = requestUrl(input);
    const method = (init?.method ?? "GET").toString();
    const body = parseBody(init?.body);
    calls.push({ url, method, headers: headersToRecord(init?.headers), body });
    if (url === "https://example.test/api/sessions" && method === "POST") {
      return new Response(JSON.stringify({ session: { id: "sess_property", status: "idle", acceptsMessages: true } }), {
        status: 201,
        headers: { "content-type": "application/json" }
      });
    }
    throw new Error(`unexpected SDK network call: ${method} ${url}`);
  };
  return {
    client: new Aex({
      apiKey: "tkn_property",
      baseUrl: "https://example.test",
      fetch: fetchImpl,
      retry: false
    }),
    calls
  };
}

function requestUrl(input: Parameters<FetchLike>[0]): string {
  if (typeof input === "string") return input;
  if (input instanceof URL) return input.toString();
  return input.url;
}

function parseBody(body: BodyInit | null | undefined): unknown {
  if (typeof body !== "string") return body;
  return JSON.parse(body);
}

function headersToRecord(input: HeadersInit | undefined): Record<string, string> {
  const headers: Record<string, string> = {};
  if (input instanceof Headers) {
    for (const [key, value] of input.entries()) headers[key.toLowerCase()] = value;
    return headers;
  }
  if (Array.isArray(input)) {
    for (const [key, value] of input) headers[key.toLowerCase()] = value;
    return headers;
  }
  for (const [key, value] of Object.entries(input ?? {})) headers[key.toLowerCase()] = value;
  return headers;
}

async function createWithSurface(
  client: Aex,
  surface: CreateSurface,
  options: SessionCreateOptions
): Promise<unknown> {
  void surface;
  return client.sessions.create(options);
}

function validateAcceptedCreate({ calls }: CaptureHarness, testCase: ValidCase): void {
  expect(calls).toHaveLength(1);
  const call = calls[0]!;
  expect(call.url).toBe("https://example.test/api/sessions");
  expect(call.method).toBe("POST");
  expect(call.headers.authorization).toBe("Bearer tkn_property");
  const idempotencyHeader = call.headers["idempotency-key"];
  expect(typeof idempotencyHeader).toBe("string");
  if (testCase.options.idempotencyKey !== undefined) {
    expect(idempotencyHeader).toBe(testCase.options.idempotencyKey);
  } else {
    expect(idempotencyHeader).not.toBe("");
  }

  const body = requireRecord(call.body, "session create body");
  expect("idempotencyKey" in body).toBe(false);
  expect(body.retention).toEqual({ idleTtl: testCase.options.overrides?.idleTtl ?? "3m" });

  const submission = requireRecord(body.submission, "session create submission");
  expect("prompt" in submission).toBe(false);
  const { retention: _retention, ...runSubmissionFrame } = body;
  void _retention;

  const parsed = parseSessionSubmissionRequest({
    ...runSubmissionFrame,
    workspaceId: "ws_property",
    idempotencyKey: idempotencyHeader,
    submission: {
      ...submission,
      prompt: ["session-create property prompt"]
    }
  });

  expect(parsed.provider).toBe(testCase.resolvedProvider);
  expect(parsed.submission.model).toBe(testCase.options.model);
  expect(parsed.secrets.apiKeys?.[testCase.resolvedProvider]).toBe(
    testCase.options.apiKeys?.[testCase.resolvedProvider]
  );
}

function requireRecord(input: unknown, label: string): Record<string, unknown> {
  expect(input, label).toBeTypeOf("object");
  expect(input, label).not.toBeNull();
  expect(Array.isArray(input), label).toBe(false);
  return input as Record<string, unknown>;
}

function buildSessionOptions(
  choice: ProviderChoice,
  parts: {
    readonly apiKeys?: Partial<Record<ProviderName, string>> | undefined;
    readonly system?: string | undefined;
    readonly metadata?: Record<string, JsonValue> | undefined;
    readonly environment?: SessionCreateOptions["environment"] | undefined;
    readonly fileCapture?: SessionCreateOptions["fileCapture"] | undefined;
    readonly overrides?: SessionCreateOptions["overrides"] | undefined;
    readonly runtime?: SessionRuntime | undefined;
    readonly outputMode?: OutputMode | undefined;
    readonly builtinTools?: "default" | "none" | readonly BuiltinToolName[] | undefined;
    readonly webhook?: { readonly url: string } | undefined;
    readonly idempotencyKey?: string | undefined;
  }
): SessionCreateOptions {
  const options: Mutable<SessionCreateOptions> = { model: choice.model };
  if (choice.provider !== undefined) options.provider = choice.provider;
  if (parts.apiKeys !== undefined) options.apiKeys = parts.apiKeys;
  if (parts.system !== undefined) options.system = parts.system;
  if (parts.metadata !== undefined) options.metadata = parts.metadata;
  if (parts.environment !== undefined) options.environment = parts.environment;
  if (parts.fileCapture !== undefined) options.fileCapture = parts.fileCapture;
  if (parts.overrides !== undefined) options.overrides = parts.overrides;
  if (parts.runtime !== undefined) options.runtime = parts.runtime;
  if (parts.outputMode !== undefined) options.outputMode = parts.outputMode;
  if (parts.builtinTools !== undefined) options.builtinTools = parts.builtinTools;
  if (parts.webhook !== undefined) options.webhook = parts.webhook;
  if (parts.idempotencyKey !== undefined) options.idempotencyKey = parts.idempotencyKey;
  return options;
}

function buildEnvironment(parts: {
  readonly variables?: Readonly<Record<string, string>> | undefined;
  readonly secrets?: Readonly<Record<string, Secret>> | undefined;
}): SessionCreateOptions["environment"] {
  const environment: Mutable<NonNullable<SessionCreateOptions["environment"]>> = {};
  if (parts.variables !== undefined) environment.variables = parts.variables;
  if (parts.secrets !== undefined) environment.secrets = parts.secrets;
  return environment;
}

function buildFileCapture(parts: {
  readonly allowedDirs?: readonly string[] | undefined;
  readonly deniedDirs?: readonly string[] | undefined;
  readonly captureTimeoutMs?: number | undefined;
  readonly maxFileBytes?: number | undefined;
  readonly maxTotalBytes?: number | undefined;
  readonly maxFiles?: number | undefined;
}): NonNullable<SessionCreateOptions["fileCapture"]> {
  const fileCapture: Mutable<NonNullable<SessionCreateOptions["fileCapture"]>> = {};
  if (parts.allowedDirs !== undefined) fileCapture.allowedDirs = parts.allowedDirs;
  if (parts.deniedDirs !== undefined) fileCapture.deniedDirs = parts.deniedDirs;
  if (parts.captureTimeoutMs !== undefined) fileCapture.captureTimeoutMs = parts.captureTimeoutMs;
  if (parts.maxFileBytes !== undefined) fileCapture.maxFileBytes = parts.maxFileBytes;
  if (parts.maxTotalBytes !== undefined) fileCapture.maxTotalBytes = parts.maxTotalBytes;
  if (parts.maxFiles !== undefined) fileCapture.maxFiles = parts.maxFiles;
  return fileCapture;
}

function buildOverrides(parts: {
  readonly idleTtl?: string | undefined;
  readonly timeout?: string | undefined;
  readonly maxSpendUsd?: number | undefined;
}): NonNullable<SessionCreateOptions["overrides"]> {
  const overrides: Mutable<NonNullable<SessionCreateOptions["overrides"]>> = {};
  if (parts.idleTtl !== undefined) overrides.idleTtl = parts.idleTtl;
  if (parts.timeout !== undefined) overrides.timeout = parts.timeout;
  if (parts.maxSpendUsd !== undefined) overrides.maxSpendUsd = parts.maxSpendUsd;
  return overrides;
}

function apiKeysFor(provider: ProviderName): fc.Arbitrary<Partial<Record<ProviderName, string>>> {
  const otherProviders = PROVIDERS.filter((entry) => entry !== provider);
  return fc
    .tuple(safeToken, fc.uniqueArray(fc.constantFrom(...otherProviders), { maxLength: 3 }))
    .map(([token, extras]) => {
      const apiKeys: Partial<Record<ProviderName, string>> = {
        [provider]: `fake-${provider}-${token}`
      };
      for (const extra of extras) {
        apiKeys[extra] = `fake-${extra}-${token}`;
      }
      return apiKeys;
    });
}

function minimalValidOptions(): SessionCreateOptions {
  return {
    model: Models.CLAUDE_HAIKU_4_5,
    apiKeys: { anthropic: "fake-anthropic-key" }
  };
}

// bun's describe() takes no options object; this file-wide default replaces the
// former vitest describe-level { timeout: 30_000 } (single suite spans the file).
setDefaultTimeout(30_000);

describe("session create inputs (property)", () => {
  it("posts rich valid sessions.create options as parseable session submissions", async () => {
    await fc.assert(
      fc.asyncProperty(richValidCase, async (testCase) => {
        const harness = captureClient();
        await createWithSurface(harness.client, testCase.surface, testCase.options);
        validateAcceptedCreate(harness, testCase);
      }),
      { numRuns: 180 }
    );
  });

  it("posts sparse valid option combinations exactly once and preserves session framing", async () => {
    await fc.assert(
      fc.asyncProperty(sparseValidCase, async (testCase) => {
        const harness = captureClient();
        await createWithSurface(harness.client, testCase.surface, testCase.options);
        validateAcceptedCreate(harness, testCase);
      }),
      { numRuns: 220 }
    );
  });

  it("rejects removed launch-era fields before any HTTP call", async () => {
    for (const field of legacyFields) {
      await fc.assert(
        fc.asyncProperty(createSurface, legacyValue, async (surface, value) => {
          const harness = captureClient();
          await expect(
            createWithSurface(harness.client, surface, {
              ...minimalValidOptions(),
              [field]: value
            } as unknown as SessionCreateOptions)
          ).rejects.toBeInstanceOf(Error);
          expect(harness.calls, field).toHaveLength(0);
        }),
        { numRuns: 20 }
      );
    }
  });

  it("rejects provider/model mismatches and missing provider keys before HTTP", async () => {
    await fc.assert(
      fc.asyncProperty(createSurface, providerMismatch, async (surface, options) => {
        const harness = captureClient();
        await expect(
          createWithSurface(harness.client, surface, options)
        ).rejects.toBeInstanceOf(SessionConfigValidationError);
        expect(harness.calls).toHaveLength(0);
      }),
      { numRuns: 120 }
    );

    await fc.assert(
      fc.asyncProperty(createSurface, missingProviderKey, async (surface, options) => {
        const harness = captureClient();
        await expect(
          createWithSurface(harness.client, surface, options)
        ).rejects.toBeInstanceOf(SessionConfigValidationError);
        expect(harness.calls).toHaveLength(0);
      }),
      { numRuns: 120 }
    );
  });
});
