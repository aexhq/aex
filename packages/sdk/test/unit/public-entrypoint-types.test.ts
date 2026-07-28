import { describe, expect, it } from "bun:test";
import {
  SessionConfigValidationError
} from "../../src/index.js";
import type {
  Aex,
  ChildSessionHandle,
  SessionClient,
  SessionHandle,
  SessionRunStream,
  Message,
  SessionCreateOptions,
  SessionEnvironmentOptions,
  SessionInput,
  SessionOverrides,
  SessionRunResult,
  SessionSendOptions,
  SessionStartOptions,
  StartSessionOptions,
  WorkspaceClient,
  WorkspaceFilesClient,
  WorkspaceInstructionsClient,
  WorkspaceSkillsClient,
  WorkspaceToolsClient,
  SessionResult
} from "../../src/index.js";
import type {
  Message as ClientMessage,
  SessionCreateOptions as ClientSessionCreateOptions,
  SessionEnvironmentOptions as ClientSessionEnvironmentOptions,
  SessionInput as ClientSessionInput,
  SessionOverrides as ClientSessionOverrides,
  SessionResult as ClientSessionResult,
  SessionRunResult as ClientSessionRunResult,
  SessionSendOptions as ClientSessionSendOptions,
  SessionStartOptions as ClientSessionStartOptions,
  StartSessionOptions as ClientStartSessionOptions
} from "../../src/client.js";

// Type-only: a value import of a missing export is a hard runtime SyntaxError
// under bun; the runtime absence is proven below via dynamic import.
// @ts-expect-error The validation adapter is a package-private implementation detail.
import type { validatedSessionConfig } from "../../src/index.js";
// @ts-expect-error Diagnostic policy is not part of the supported SDK type surface.
import type { SessionConfigDiagnosticPolicy } from "../../src/index.js";
// @ts-expect-error Exact option-key proofs are private validation machinery.
import type { ExactKeySet } from "../../src/index.js";
// @ts-expect-error The aggregate equality proof is private validation machinery.
import type { SessionOptionKeyAssertions } from "../../src/index.js";
// @ts-expect-error The start-options validator is not a supported SDK export.
import type { assertStartSessionOptions } from "../../src/index.js";

// @ts-expect-error Raw platform submission envelopes are not SDK input types.
import type { PlatformSessionSubmissionRequest } from "../../src/index.js";
// @ts-expect-error SessionRecord is a retired transport projection.
import type { SessionRecord } from "../../src/index.js";
// @ts-expect-error SessionEvent is replaced by the canonical Aex event surface.
import type { SessionEvent } from "../../src/index.js";
// @ts-expect-error ObservableSessionRef is an internal composition helper.
import type { ObservableSessionRef } from "../../src/index.js";
// @ts-expect-error ProviderEvent exposes an upstream transport detail.
import type { ProviderEvent } from "../../src/index.js";
// @ts-expect-error FileRecordWire is an obsolete wire alias.
import type { FileRecordWire } from "../../src/index.js";
// @ts-expect-error SkillRecordWire is an obsolete wire alias.
import type { SkillRecordWire } from "../../src/index.js";
// @ts-expect-error The idempotency key is SDK-owned; callers no longer supply or manage one.
import type { IdempotencyOptions } from "../../src/index.js";

type ReturnSurface =
  | ChildSessionHandle
  | SessionClient
  | SessionHandle
  | SessionRunStream
  | WorkspaceClient
  | WorkspaceFilesClient
  | WorkspaceInstructionsClient
  | WorkspaceSkillsClient
  | WorkspaceToolsClient;

type RemovedSurface =
  | PlatformSessionSubmissionRequest
  | SessionRecord
  | SessionEvent
  | ObservableSessionRef
  | ProviderEvent
  | FileRecordWire
  | SkillRecordWire
  | IdempotencyOptions;

void (undefined as unknown as ReturnSurface);
void (undefined as unknown as RemovedSurface);
void (undefined as unknown as SessionConfigDiagnosticPolicy);
void (undefined as unknown as ExactKeySet<object, readonly []>);
void (undefined as unknown as SessionOptionKeyAssertions);

const legacyValidationError = new SessionConfigValidationError("invalid", { field: "runtime.size" });
const diagnosticValidationError = new SessionConfigValidationError(
  "invalid",
  { field: "runtime.size" },
  { cause: new Error("bounded diagnostic") }
);
type SessionConfigValidationDetails = ConstructorParameters<typeof SessionConfigValidationError>[1];
const validationDetailsHasOnlyField: Equal<keyof SessionConfigValidationDetails, "field"> = true;
void [legacyValidationError, diagnosticValidationError, validationDetailsHasOnlyField];

const sessionResultHasNoRecord: "record" extends keyof SessionResult ? false : true = true;
const sessionResultRequiresSession: {} extends Pick<SessionResult, "session"> ? false : true = true;
const sessionResultRequiresRun: {} extends Pick<SessionResult, "run"> ? false : true = true;
const aexHasNoRawUploadMethod: "_uploadAsset" extends keyof Aex ? false : true = true;
const aexHasNoRawStreamUploadMethod: "_uploadAssetStream" extends keyof Aex ? false : true = true;
const aexHasNoSecretPromotionMethod: "_createWorkspaceSecret" extends keyof Aex ? false : true = true;
const sessionSendHasNoReplayCursor: "from" extends keyof SessionSendOptions ? false : true = true;
const sessionSendHasNoSignal: "signal" extends keyof SessionSendOptions ? false : true = true;
const sessionStreamHasNoIdempotencyKey:
  "idempotencyKey" extends keyof NonNullable<SessionStartOptions["stream"]> ? false : true = true;
// The idempotency key is SDK-owned. It must not reappear on ANY public option
// shape: a caller-managed key is the bad-DX surface this release removed, and a
// caller-driven retry loop is explicitly not covered by the dedup guarantee.
const sessionCreateHasNoIdempotencyKey:
  "idempotencyKey" extends keyof SessionCreateOptions ? false : true = true;
const sessionSendHasNoIdempotencyKey:
  "idempotencyKey" extends keyof SessionSendOptions ? false : true = true;
const sessionStartHasNoIdempotencyKey:
  "idempotencyKey" extends keyof SessionStartOptions ? false : true = true;
const sessionStartHasNoMessageIdempotencyKey:
  "messageIdempotencyKey" extends keyof SessionStartOptions ? false : true = true;
const environmentHasNoWireEnvVars:
  "envVars" extends keyof SessionEnvironmentOptions ? false : true = true;
type SessionPackageInput = NonNullable<SessionEnvironmentOptions["packages"]>[number];
const packageInputHasNoParsedEcosystem:
  "ecosystem" extends keyof SessionPackageInput ? false : true = true;
type Equal<Left, Right> =
  (<T>() => T extends Left ? 1 : 2) extends (<T>() => T extends Right ? 1 : 2)
    ? (<T>() => T extends Right ? 1 : 2) extends (<T>() => T extends Left ? 1 : 2)
      ? true
      : false
    : false;
const movedTypeCompatibility: readonly true[] = [
  true as Equal<Message, ClientMessage>,
  true as Equal<SessionCreateOptions, ClientSessionCreateOptions>,
  true as Equal<SessionEnvironmentOptions, ClientSessionEnvironmentOptions>,
  true as Equal<SessionInput, ClientSessionInput>,
  true as Equal<SessionOverrides, ClientSessionOverrides>,
  true as Equal<SessionResult, ClientSessionResult>,
  true as Equal<SessionRunResult, ClientSessionRunResult>,
  true as Equal<SessionSendOptions, ClientSessionSendOptions>,
  true as Equal<SessionStartOptions, ClientSessionStartOptions>,
  true as Equal<StartSessionOptions, ClientStartSessionOptions>
];
void [
  sessionResultHasNoRecord,
  sessionResultRequiresSession,
  sessionResultRequiresRun,
  aexHasNoRawUploadMethod,
  aexHasNoRawStreamUploadMethod,
  aexHasNoSecretPromotionMethod,
  sessionSendHasNoReplayCursor,
  sessionSendHasNoSignal,
  sessionStreamHasNoIdempotencyKey,
  sessionCreateHasNoIdempotencyKey,
  sessionSendHasNoIdempotencyKey,
  sessionStartHasNoIdempotencyKey,
  sessionStartHasNoMessageIdempotencyKey,
  environmentHasNoWireEnvVars,
  packageInputHasNoParsedEcosystem,
  movedTypeCompatibility
];

describe("SDK root type boundary", () => {
  it("keeps implementation classes usable as inferred return types only", async () => {
    const sdk = await import("../../src/index.js");
    expect(sdk.Aex).toBeTypeOf("function");
    expect(sdk).not.toHaveProperty("SessionClient");
    expect(sdk).not.toHaveProperty("WorkspaceClient");
    expect(sdk).not.toHaveProperty("assertStartSessionOptions");
    expect(sdk).not.toHaveProperty("ExactKeySet");
  });
});
