/**
 * Schema for the session-submission REQUEST envelope — the outermost object a
 * platform adapter hands {@link parseSessionSubmissionRequest}: workspace and
 * idempotency identity, the submission brief itself, the runtime dials, and the
 * vaulted `secrets` side-channel.
 *
 * Every member mounts the schema that already owns its vocabulary, so the
 * envelope is one parse rather than a key gate followed by ten. What stays on
 * the parser below is what a schema cannot state: the duration string is
 * DECODED to milliseconds and bounded there, the vaulted bundle and the brief
 * are cross-checked against each other, and the credential-named-field sweep
 * runs over the parsed value.
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
import { RuntimeKindSchema } from "./runtime-kind.js";
import { RuntimeSizeSchema, SessionTimeoutSchema } from "./runtime-sizes.js";
import { SessionLimitsSchema } from "./session-limits.js";
import { SessionMachineSchema } from "./session-machine.js";
import { SessionWebhookSchema } from "./session-webhook.js";
import { SubmissionSchema } from "./submission-body.js";
import { InlineSecretsSchema } from "./submission-secrets.js";
import { nonEmptyString, wireObject } from "./wire.js";

const REQUEST = "submission";

/** Wire shape of the submission request envelope. */
export const SessionSubmissionRequestSchema = wireObject(REQUEST, {
  workspaceId: nonEmptyString("workspaceId"),
  idempotencyKey: nonEmptyString("idempotencyKey"),
  submission: SubmissionSchema,
  runtimeSize: z.optional(RuntimeSizeSchema),
  runtimeKind: z.optional(RuntimeKindSchema),
  // A string here, a bounded millisecond deadline after `parseSessionTimeout`
  // decodes it: `"90m"` and `"5400000"` are the same deadline and only the
  // decoded value can be judged against the accepted window.
  timeout: z.optional(SessionTimeoutSchema),
  webhook: z.optional(SessionWebhookSchema),
  limits: z.optional(SessionLimitsSchema),
  machine: z.optional(SessionMachineSchema),
  // Absent or null collapses to an empty bundle: under managed gateway keys a
  // run needs no provider key, so an empty bundle is always admissible.
  secrets: z.optional(z.nullable(InlineSecretsSchema))
});

export type SessionSubmissionRequestWire = z.infer<typeof SessionSubmissionRequestSchema>;
