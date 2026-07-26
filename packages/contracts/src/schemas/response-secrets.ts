/**
 * Response schemas for the `secrets.*` family — the workspace secret store.
 *
 * These records are METADATA ONLY by design: the persisted value is write-only
 * through this API. A strict object is the check that keeps it that way — if a
 * `value`, `envelope` or `ciphertext` key ever appears on a read, the suite
 * fails rather than the value quietly reaching a log.
 *
 * Divergence from the declared `SecretRecord`: that interface makes `createdAt`
 * and `updatedAt` optional and adds `deletedAt?: string | null`. The server
 * sends all six fields unconditionally and never a `deletedAt`. `deletedAt` is
 * kept (our own type declares it). The index signature that interface also
 * carried is gone — see `response-common.ts`.
 *
 * `createdAt` / `updatedAt` are ISO-8601 with a `Z`, unlike the billing
 * timestamps, which are raw Data-API text.
 */
import * as z from "zod/mini";
import {
  describeResponse,
  responseObject,
  wireLiteral,
  wireNonEmptyString,
  wirePositiveInteger,
  wireString
} from "./response-common.js";

const optional = z.optional;

export const SecretRecordSchema = describeResponse(
  "SecretRecord",
  "Metadata for one workspace secret. Never carries the value.",
  responseObject({
    id: wireNonEmptyString,
    name: wireNonEmptyString,
    version: wirePositiveInteger,
    state: wireLiteral("ready"),
    createdAt: optional(wireString),
    updatedAt: optional(wireString),
    deletedAt: optional(z.nullable(wireString))
  })
);

/** `POST /secrets` (201), `GET /secrets/{name}` (200), `POST /secrets/{name}/rotate` (200). */
export const SecretResponseSchema = describeResponse(
  "SecretResponse",
  "One workspace secret's metadata.",
  responseObject({ secret: SecretRecordSchema })
);

export const SecretListResponseSchema = describeResponse(
  "SecretListResponse",
  "Every workspace secret's metadata, ordered by name. Not paged.",
  responseObject({ secrets: z.array(SecretRecordSchema) })
);
