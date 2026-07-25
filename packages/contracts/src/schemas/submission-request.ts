/**
 * Schema for the session-submission REQUEST envelope — the outermost object a
 * platform adapter hands {@link parseSessionSubmissionRequest}: workspace and
 * idempotency identity, the submission brief itself, the runtime dials, and the
 * vaulted `secrets` side-channel.
 *
 * **Key gate only.** Every member is {@link unspecifiedField}: the KEYS are now
 * declared once, here, but each value is still validated below the gate by the
 * parser that owns its vocabulary (`parseRuntimeSize`, `parseSessionTimeout`,
 * `parseInlineSecrets`, `parseSubmission`, …). That is the migration state D4/L1
 * describes — a known gap in the generated OpenAPI document, closed as each
 * member's own schema lands.
 *
 * Declaration order is the permitted-key order reported on an unknown field, and
 * `test/allowed-keys-parser-golden.test.ts` pins the resulting sentence.
 *
 * The wire path is `submission`, not `request`: the envelope has always reported
 * itself under that name (`submission.workspaceId is not an allowed field`).
 * `timeout` is the wire duration string; the parsed request carries `timeoutMs`,
 * so the two never appear in the same shape.
 */
import * as z from "zod/mini";
import { unspecifiedField, wireObject } from "./wire.js";

const REQUEST = "submission";

/** Wire shape of the submission request envelope. */
export const SessionSubmissionRequestSchema = wireObject(REQUEST, {
  workspaceId: unspecifiedField,
  idempotencyKey: unspecifiedField,
  submission: unspecifiedField,
  runtimeSize: unspecifiedField,
  runtimeKind: unspecifiedField,
  timeout: unspecifiedField,
  webhook: unspecifiedField,
  limits: unspecifiedField,
  machine: unspecifiedField,
  secrets: unspecifiedField
});

export type SessionSubmissionRequestWire = z.infer<typeof SessionSubmissionRequestSchema>;
