import { describe, expect, it } from "vitest";
import type {
  Aex,
  ChildSessionHandle,
  SessionClient,
  SessionHandle,
  SessionRunStream,
  WorkspaceClient,
  WorkspaceFilesClient,
  WorkspaceInstructionsClient,
  WorkspaceSkillsClient,
  WorkspaceToolsClient,
  SessionResult
} from "../../src/index.js";

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
void [
  sessionResultHasNoRecord,
  sessionResultRequiresSession,
  sessionResultRequiresRun,
  aexHasNoRawUploadMethod,
  aexHasNoRawStreamUploadMethod,
  aexHasNoSecretPromotionMethod
];

describe("SDK root type boundary", () => {
  it("keeps implementation classes usable as inferred return types only", async () => {
    const sdk = await import("../../src/index.js");
    expect(sdk.Aex).toBeTypeOf("function");
    expect(sdk).not.toHaveProperty("SessionClient");
    expect(sdk).not.toHaveProperty("WorkspaceClient");
  });
});
