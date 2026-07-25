/**
 * Schemas for the submission BRIEF — `request.submission` — and for the inline
 * policy objects that hang directly off it: platform injection, file capture,
 * response format and the HITL approval gate.
 *
 * **Key gate only.** Every member is {@link unspecifiedField}: the KEYS are
 * declared once here, while the value rules still live in `submission.ts`
 * (D4/L1). Declaration order is the permitted-key order reported on an unknown
 * field, and `test/allowed-keys-parser-golden.test.ts` pins every sentence below
 * byte-for-byte — including the three different wordings, which are historical
 * and deliberately not normalised toward each other.
 */
import * as z from "zod/mini";
import { unspecifiedField, wireObject } from "./wire.js";

const SUBMISSION = "submission";
const PLATFORM = `${SUBMISSION}.platform`;
const FILE_CAPTURE = `${SUBMISSION}.fileCapture`;
const RESPONSE_FORMAT = `${SUBMISSION}.responseFormat`;
const APPROVAL_GATE = `${SUBMISSION}.approvalGate`;

/**
 * Wire shape of the submission brief.
 *
 * The two diagnostics report under DIFFERENT paths and always have: an unknown
 * key is `submission.<key> …` (the brief's fields are the caller's
 * `submission.*` fields), while a non-object brief is `submission.submission …`
 * (the brief's own position inside the request envelope). Both are pinned.
 */
export const SubmissionSchema = wireObject(
  SUBMISSION,
  {
    model: unspecifiedField,
    system: unspecifiedField,
    prompt: unspecifiedField,
    assets: unspecifiedField,
    mcpServers: unspecifiedField,
    secretEnv: unspecifiedField,
    environment: unspecifiedField,
    securityProfile: unspecifiedField,
    metadata: unspecifiedField,
    fileCapture: unspecifiedField,
    builtinTools: unspecifiedField,
    outputMode: unspecifiedField,
    responseFormat: unspecifiedField,
    approvalGate: unspecifiedField,
    platform: unspecifiedField
  },
  { notObject: () => `${SUBMISSION}.submission must be an object` }
);

export type SubmissionWire = z.infer<typeof SubmissionSchema>;

/** `submission.platform` — platform-injection controls. */
export const PlatformInjectionSchema = wireObject(PLATFORM, {
  systemPrompt: unspecifiedField
});

/** `submission.fileCapture` — the post-run capture policy. */
export const FileCaptureSchema = wireObject(FILE_CAPTURE, {
  allowedDirs: unspecifiedField,
  deniedDirs: unspecifiedField,
  captureTimeoutMs: unspecifiedField,
  maxFileBytes: unspecifiedField,
  maxTotalBytes: unspecifiedField,
  maxFiles: unspecifiedField
});

/**
 * `submission.responseFormat` when `kind` is `"text"` — a free-form response
 * admits no other field, and says so in its own words rather than listing a
 * permitted set of one.
 *
 * The variant is selected by `kind` BEFORE either schema runs, so `kind` is
 * declared but not re-validated here; the parser has already rejected anything
 * outside {@link RESPONSE_FORMAT_KINDS}.
 */
export const ResponseFormatTextSchema = wireObject(
  RESPONSE_FORMAT,
  { kind: unspecifiedField },
  { unknownKey: (path, key) => `${path}.${key} is not allowed when kind is 'text'` }
);

/** `submission.responseFormat` when `kind` is `"json_schema"`. */
export const ResponseFormatJsonSchemaSchema = wireObject(RESPONSE_FORMAT, {
  kind: unspecifiedField,
  schema: unspecifiedField,
  strict: unspecifiedField,
  name: unspecifiedField
});

/** `submission.approvalGate` — the declarative HITL write-gate. */
export const ApprovalGateSchema = wireObject(APPROVAL_GATE, {
  tools: unspecifiedField
});
