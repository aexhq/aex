import { describe, expect, it } from "vitest";
import type {
  Aex,
  ChildSessionHandle,
  SessionClient,
  SessionHandle,
  SessionRunStream,
  IdempotencyOptions,
  Message,
  SessionCreateOptions,
  SessionEnvironmentOptions,
  SessionInput,
  SessionOverrides,
  SessionRunResult,
  SessionSendOptions,
  SessionStartOptions,
  WorkspaceClient,
  WorkspaceFilesClient,
  WorkspaceInstructionsClient,
  WorkspaceSkillsClient,
  WorkspaceToolsClient,
  SessionResult
} from "../../src/index.js";
import type {
  IdempotencyOptions as ClientIdempotencyOptions,
  Message as ClientMessage,
  SessionCreateOptions as ClientSessionCreateOptions,
  SessionEnvironmentOptions as ClientSessionEnvironmentOptions,
  SessionInput as ClientSessionInput,
  SessionOverrides as ClientSessionOverrides,
  SessionResult as ClientSessionResult,
  SessionRunResult as ClientSessionRunResult,
  SessionSendOptions as ClientSessionSendOptions,
  SessionStartOptions as ClientSessionStartOptions
} from "../../src/client.js";

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
  | SkillRecordWire;

void (undefined as unknown as ReturnSurface);
void (undefined as unknown as RemovedSurface);

const sessionResultHasNoRecord: "record" extends keyof SessionResult ? false : true = true;
const sessionResultRequiresSession: {} extends Pick<SessionResult, "session"> ? false : true = true;
const sessionResultRequiresRun: {} extends Pick<SessionResult, "run"> ? false : true = true;
const aexHasNoRawUploadMethod: "_uploadAsset" extends keyof Aex ? false : true = true;
const aexHasNoRawStreamUploadMethod: "_uploadAssetStream" extends keyof Aex ? false : true = true;
const aexHasNoSecretPromotionMethod: "_createWorkspaceSecret" extends keyof Aex ? false : true = true;
const sessionSendHasNoReplayCursor: "from" extends keyof SessionSendOptions ? false : true = true;
type Equal<Left, Right> =
  (<T>() => T extends Left ? 1 : 2) extends (<T>() => T extends Right ? 1 : 2)
    ? (<T>() => T extends Right ? 1 : 2) extends (<T>() => T extends Left ? 1 : 2)
      ? true
      : false
    : false;
const movedTypeCompatibility: readonly true[] = [
  true as Equal<IdempotencyOptions, ClientIdempotencyOptions>,
  true as Equal<Message, ClientMessage>,
  true as Equal<SessionCreateOptions, ClientSessionCreateOptions>,
  true as Equal<SessionEnvironmentOptions, ClientSessionEnvironmentOptions>,
  true as Equal<SessionInput, ClientSessionInput>,
  true as Equal<SessionOverrides, ClientSessionOverrides>,
  true as Equal<SessionResult, ClientSessionResult>,
  true as Equal<SessionRunResult, ClientSessionRunResult>,
  true as Equal<SessionSendOptions, ClientSessionSendOptions>,
  true as Equal<SessionStartOptions, ClientSessionStartOptions>
];
void [
  sessionResultHasNoRecord,
  sessionResultRequiresSession,
  sessionResultRequiresRun,
  aexHasNoRawUploadMethod,
  aexHasNoRawStreamUploadMethod,
  aexHasNoSecretPromotionMethod,
  sessionSendHasNoReplayCursor,
  movedTypeCompatibility
];

describe("SDK root type boundary", () => {
  it("keeps implementation classes usable as inferred return types only", async () => {
    const sdk = await import("../../src/index.js");
    expect(sdk.Aex).toBeTypeOf("function");
    expect(sdk).not.toHaveProperty("SessionClient");
    expect(sdk).not.toHaveProperty("WorkspaceClient");
  });
});
