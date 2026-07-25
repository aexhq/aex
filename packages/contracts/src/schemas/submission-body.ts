/**
 * Schemas for the submission BRIEF — `request.submission` — and for the inline
 * policy objects that hang directly off it: platform injection, file capture,
 * response format and the HITL approval gate.
 *
 * Declaration order is the permitted-key order reported on an unknown field, and
 * `test/allowed-keys-parser-golden.test.ts` pins every sentence below
 * byte-for-byte — including the three different wordings, which are historical
 * and deliberately not normalised toward each other.
 *
 * **What stays in `submission.ts`** (D4/L1): every transform and every
 * cross-field rule. Splitting `"pip:pandas"`, decoding a duration, collapsing an
 * empty object to `undefined`, canonicalising a capture path, deduplicating a
 * tool list, reconciling `secretEnv` against `secrets.envSecrets` — none of
 * those are shapes, and a `.transform()` here would make the response half of
 * the generated spec ungenerable. Rule sets whose ORDER decides which message
 * fires (`prompt`, `secretEnv`, the capture-path ladders) are single
 * `z.check()`s for the reason `submission-environment.ts` documents: composed
 * schemas report in Zod's order, not the ladder's.
 */
import * as z from "zod/mini";
import { isJsonValue, isRecord } from "../value-guards.js";
import { SECRET_ENV_NAME_PATTERN, SECRET_HANDLE_PATTERN } from "../submission-limits.js";
import { modelSlugSchema } from "./models.js";
import { RuntimeSecurityProfileSchema } from "./runtime-security-profile.js";
import { SubmissionAssetsSchema } from "./submission-assets.js";
import { EnvironmentSchema } from "./submission-environment.js";
import { lastSegment, nonEmptyString, wireObject } from "./wire.js";

const SUBMISSION = "submission";
const PROMPT = `${SUBMISSION}.prompt`;
const MCP_SERVERS = `${SUBMISSION}.mcpServers`;
const SECRET_ENV = `${SUBMISSION}.secretEnv`;
const METADATA = `${SUBMISSION}.metadata`;
const BUILTIN_TOOLS = `${SUBMISSION}.builtinTools`;
const PLATFORM = `${SUBMISSION}.platform`;
const FILE_CAPTURE = `${SUBMISSION}.fileCapture`;
const RESPONSE_FORMAT = `${SUBMISSION}.responseFormat`;
const APPROVAL_GATE = `${SUBMISSION}.approvalGate`;

/** Assistant-output granularity values. Buffered is the platform default. */
export const OUTPUT_MODES = ["buffered", "stream"] as const;

/** Response-format kinds: free-form `text` (default) or provider-native `json_schema`. */
export const RESPONSE_FORMAT_KINDS = ["text", "json_schema"] as const;

/**
 * Maximum number of file capture entries accepted per list.
 *
 * 32 is enough room for the typical "one or two capture roots" pattern plus a
 * generous margin for legitimate multi-root use cases (per-tool file directory +
 * scratch state + logs, repeated across a few subdirectories), without inviting
 * abuse of the synthetic-turn path the platform capture path drives at session
 * terminal.
 */
export const MAX_FILE_CAPTURE_DIRS = 32;

/**
 * Maximum byte length of a single file capture entry (after UTF-8 encoding).
 * 512 bytes comfortably covers `/very/long/nested/path` style entries without
 * letting a misuse smuggle large blobs through the field.
 */
export const MAX_FILE_CAPTURE_DIR_BYTES = 512;

function rejector(payload: { readonly value: unknown; readonly issues: unknown[] }) {
  return (message: string): void => {
    payload.issues.push({ code: "custom", message, input: payload.value });
  };
}

// ---------------------------------------------------------------------------
// prompt
// ---------------------------------------------------------------------------

/**
 * The prompt as a single string or an ordered list of parts.
 *
 * The union admits ANY array rather than an array of strings, and the per-entry
 * rules ride the ladder below, because a union reports its own failure — the
 * caller would learn "must be a string or an array of strings" for
 * `["ok", 7]` instead of which entry is wrong. The ladder is the parser's own
 * order: length, then each entry, then the all-whitespace check that only makes
 * sense once every entry is a string.
 */
export const PromptSchema = z
  .union([z.string(), z.array(z.unknown())], {
    error: `${PROMPT} must be a string or an array of strings`
  })
  .check(
    z.check((payload) => {
      const reject = rejector(payload);
      const value = payload.value;
      if (typeof value === "string") {
        if (value.length === 0) {
          reject(`${PROMPT} must be non-empty`);
        } else if (value.trim().length === 0) {
          reject(`${PROMPT} must contain non-whitespace text`);
        }
        return;
      }
      const parts = value as readonly unknown[];
      if (parts.length === 0) {
        reject(`${PROMPT} array must be non-empty`);
        return;
      }
      for (const [index, part] of parts.entries()) {
        if (typeof part !== "string" || part.length === 0) {
          reject(`${PROMPT}[${index}] must be a non-empty string`);
          return;
        }
      }
      if (parts.every((part) => (part as string).trim().length === 0)) {
        reject(`${PROMPT} must contain non-whitespace text`);
      }
    })
  )
  .register(z.globalRegistry, {
    id: "SubmissionPrompt",
    description:
      "The run brief: one string, or an ordered list of non-empty parts. At least one part must " +
      "carry non-whitespace text. A single string is normalised to a one-element list."
  });

// ---------------------------------------------------------------------------
// mcpServers
// ---------------------------------------------------------------------------

/**
 * The non-secret half of the MCP server declarations.
 *
 * Deliberately an array of unvalidated entries: the per-entry rules
 * (`mcpServerRefSchema`) need the SSRF host deny-list and the transport parser,
 * which `schemas/mcp-server.ts` takes as an injected `McpWirePolicy` because
 * they must stay inside `session-config.ts` for the cross-repo parity check —
 * and `session-config.ts` imports `submission.ts`, so this module cannot reach
 * them without a cycle. `parseMcpServers` applies them one entry at a time.
 */
export const McpServerRefsSchema = z
  .array(z.unknown(), { error: `${MCP_SERVERS} must be an array of {name, url} objects` })
  .register(z.globalRegistry, {
    id: "SubmissionMcpServers",
    description:
      "Remote MCP servers, each `{ name, url, transport? }`. Names must be unique and every URL " +
      "must clear the SSRF host deny-list — enforced in packages/contracts/src/session-config.ts."
  });

// ---------------------------------------------------------------------------
// secretEnv
// ---------------------------------------------------------------------------

/**
 * Value-free env-var secret bindings, keyed by env name.
 *
 * One ordered `z.check()` rather than a record of a union, for the reason
 * `EnvSecretsSchema` documents: each rung has its own message and the key rule
 * must beat the value rule, which a composed schema cannot promise.
 */
export const SecretEnvSchema = z
  .record(z.string(), z.unknown(), { error: `${SECRET_ENV} must be an object` })
  .check(
    z.check((payload) => {
      const reject = rejector(payload);
      for (const [envName, entry] of Object.entries(payload.value as Record<string, unknown>)) {
        if (!SECRET_ENV_NAME_PATTERN.test(envName)) {
          reject(
            `${SECRET_ENV} key "${envName}" must be a valid env var name matching ${SECRET_ENV_NAME_PATTERN.source}`
          );
          return;
        }
        const path = `${SECRET_ENV}.${envName}`;
        if (!isRecord(entry)) {
          reject(`${path} must be an object`);
          return;
        }
        const keys = Object.keys(entry);
        if (keys.length !== 1 || (!("ref" in entry) && !("ephemeral" in entry))) {
          reject(`${path} must be exactly one of { ref } or { ephemeral: true }`);
          return;
        }
        if ("ref" in entry) {
          const handle = entry.ref;
          if (typeof handle !== "string" || handle.length === 0) {
            reject(`${path}.ref must be a non-empty string`);
            return;
          }
          if (!SECRET_HANDLE_PATTERN.test(handle)) {
            reject(`${path}.ref handle must match ${SECRET_HANDLE_PATTERN.source}`);
            return;
          }
        } else if (entry.ephemeral !== true) {
          reject(`${path}.ephemeral must be the literal true`);
          return;
        }
      }
    })
  )
  .register(z.globalRegistry, {
    id: "SubmissionSecretEnv",
    description:
      "Env-var secret bindings keyed by env name. Each value is exactly one of `{ ref }` (a " +
      `workspace secret handle matching ${SECRET_HANDLE_PATTERN.source}) or ` +
      "`{ ephemeral: true }` (the value rides in `secrets.envSecrets`). Enforced in " +
      "packages/contracts/src/schemas/submission-body.ts."
  });

// ---------------------------------------------------------------------------
// metadata
// ---------------------------------------------------------------------------

/** Caller-owned metadata: any JSON-serializable value per key. */
export const MetadataSchema = z.record(
  z.string(),
  z.unknown().check(
    z.refine(isJsonValue, {
      error: (issue) => `${METADATA}.${lastSegment(issue.path)} must be JSON-serializable`,
      abort: true
    })
  ),
  { error: `${METADATA} must be an object` }
);

// ---------------------------------------------------------------------------
// builtinTools
// ---------------------------------------------------------------------------

/**
 * The builtin-capability selection.
 *
 * The array branch admits any entry: the closed name set lives on
 * `submission.ts` (`BUILTIN_TOOL_NAMES`, pinned equal to the platform's
 * `HANDS_TOOLS`), which imports this module, and `resolveBuiltinToolNames` both
 * validates membership and re-orders the result — a transform either way.
 */
export const BuiltinToolsSchema = z.union(
  [z.literal("default"), z.literal("none"), z.array(z.unknown())],
  { error: `${BUILTIN_TOOLS} must be 'default', 'none', or an array of builtin tool names` }
);

// ---------------------------------------------------------------------------
// outputMode
// ---------------------------------------------------------------------------

/** Assistant-output granularity. */
export const OutputModeSchema = z.enum(OUTPUT_MODES, {
  error: `${SUBMISSION}.outputMode must be one of ${OUTPUT_MODES.join(", ")}`
});

// ---------------------------------------------------------------------------
// fileCapture
// ---------------------------------------------------------------------------

function utf8Bytes(value: string): number {
  return new TextEncoder().encode(value).length;
}

/**
 * The shared entry rules for a capture list, in the order the parser applied
 * them: the list bound first, then each entry top to bottom. One ladder rather
 * than a composed array schema because the bound must outrank an entry
 * complaint, and every rung words itself differently.
 */
function captureDirs(field: string, emptyEntry: string, notArray: string) {
  const path = `${FILE_CAPTURE}.${field}`;
  return z.array(z.unknown(), { error: notArray }).check(
    z.check((payload) => {
      const reject = rejector(payload);
      const entries = payload.value as readonly unknown[];
      if (entries.length > MAX_FILE_CAPTURE_DIRS) {
        reject(`${path} has ${entries.length} entries; max is ${MAX_FILE_CAPTURE_DIRS}`);
        return;
      }
      for (const [index, entry] of entries.entries()) {
        const at = `${path}[${index}]`;
        if (typeof entry !== "string") {
          reject(`${at} must be a string`);
          return;
        }
        if (entry.length === 0) {
          reject(`${at} ${emptyEntry}`);
          return;
        }
        const bytes = utf8Bytes(entry);
        if (bytes > MAX_FILE_CAPTURE_DIR_BYTES) {
          reject(`${at} exceeds ${MAX_FILE_CAPTURE_DIR_BYTES} bytes (got ${bytes})`);
          return;
        }
        if (field === "allowedDirs" && !entry.startsWith("/")) {
          reject(`${at} must be an absolute UNIX path (start with '/')`);
          return;
        }
        if (entry.includes("\0")) {
          reject(`${at} must not contain NUL bytes`);
          return;
        }
        if (entry.includes("\n") || entry.includes("\r")) {
          reject(`${at} must not contain newline characters`);
          return;
        }
        if (entry.split("/").includes("..")) {
          reject(`${at} must not contain '..' segments`);
          return;
        }
      }
    })
  );
}

/** A positive integer capture bound. Clamping to the platform maximum is the parser's. */
function captureInteger(field: string) {
  const message = `${FILE_CAPTURE}.${field} must be a positive integer`;
  return z
    .number({ error: message })
    .check(
      z.refine((value: number) => Number.isInteger(value) && value > 0, {
        error: message,
        abort: true
      })
    );
}

/** `submission.fileCapture` — the post-run capture policy. */
export const FileCaptureSchema = wireObject(FILE_CAPTURE, {
  allowedDirs: z.optional(
    captureDirs(
      "allowedDirs",
      "must be a non-empty absolute UNIX path",
      `${FILE_CAPTURE}.allowedDirs must be an array of absolute UNIX paths`
    )
  ),
  deniedDirs: z.optional(
    captureDirs(
      "deniedDirs",
      "must be a non-empty pattern",
      `${FILE_CAPTURE}.deniedDirs must be an array of strings`
    )
  ),
  captureTimeoutMs: z.optional(captureInteger("captureTimeoutMs")),
  maxFileBytes: z.optional(captureInteger("maxFileBytes")),
  maxTotalBytes: z.optional(captureInteger("maxTotalBytes")),
  maxFiles: z.optional(captureInteger("maxFiles"))
}).register(z.globalRegistry, {
  id: "SubmissionFileCapture",
  description:
    "Post-run file capture policy. Entries are bounded to " +
    `${MAX_FILE_CAPTURE_DIRS} per list and ${MAX_FILE_CAPTURE_DIR_BYTES} bytes each; allowed ` +
    "roots are absolute UNIX paths and neither list may contain '..' segments. Paths are " +
    "canonicalised and deduplicated after parsing, and `captureTimeoutMs` is clamped to the " +
    "platform maximum. Enforced in packages/contracts/src/schemas/submission-body.ts."
});

// ---------------------------------------------------------------------------
// responseFormat
// ---------------------------------------------------------------------------

/**
 * `submission.responseFormat` when `kind` is `"text"` — a free-form response
 * admits no other field, and says so in its own words rather than listing a
 * permitted set of one.
 */
export const ResponseFormatTextSchema = wireObject(
  RESPONSE_FORMAT,
  { kind: z.literal("text") },
  { unknownKey: (path, key) => `${path}.${key} is not allowed when kind is 'text'` }
);

const RESPONSE_FORMAT_SCHEMA_FIELD = `${RESPONSE_FORMAT}.schema must be a JSON-serializable object`;

/** `submission.responseFormat` when `kind` is `"json_schema"`. */
export const ResponseFormatJsonSchemaSchema = wireObject(RESPONSE_FORMAT, {
  kind: z.literal("json_schema"),
  schema: z
    .record(z.string(), z.unknown(), { error: RESPONSE_FORMAT_SCHEMA_FIELD })
    .check(z.refine(isJsonValue, { error: RESPONSE_FORMAT_SCHEMA_FIELD, abort: true })),
  strict: z.optional(z.boolean({ error: `${RESPONSE_FORMAT}.strict must be a boolean` })),
  name: z.optional(nonEmptyString(`${RESPONSE_FORMAT}.name`))
});

/**
 * The structured-output policy, selected by `kind`.
 *
 * A DISCRIMINATED union rather than a plain one: `kind` picks the variant before
 * either member runs, so a bad `kind` outranks an unknown key and each variant
 * words its own rejection — which is exactly the two-stage sequencing the
 * hand-written parser had. A plain `z.union` would report its own failure for
 * both cases and lose the distinction.
 *
 * The union's own message therefore covers only what routing itself can fail on:
 * a non-object, or an object whose `kind` names no variant.
 */
export const ResponseFormatSchema = z.discriminatedUnion(
  "kind",
  [ResponseFormatTextSchema, ResponseFormatJsonSchemaSchema],
  {
    error: (issue) =>
      isRecord(issue.input)
        ? `${RESPONSE_FORMAT}.kind must be one of ${RESPONSE_FORMAT_KINDS.join(", ")}`
        : `${RESPONSE_FORMAT} must be an object`
  }
);

// ---------------------------------------------------------------------------
// approvalGate + platform
// ---------------------------------------------------------------------------

const approvalTool = (() => {
  const message = (issue: { readonly path?: readonly PropertyKey[] | undefined }): string =>
    `${APPROVAL_GATE}.tools[${lastSegment(issue.path)}] must be a non-empty string`;
  return z.string({ error: message }).check(z.minLength(1, { error: message, abort: true }));
})();

/** `submission.approvalGate` — the declarative HITL write-gate. */
export const ApprovalGateSchema = wireObject(APPROVAL_GATE, {
  tools: z.array(approvalTool, { error: `${APPROVAL_GATE}.tools must be an array of tool names` })
});

/** `submission.platform` — platform-injection controls. */
export const PlatformInjectionSchema = wireObject(PLATFORM, {
  systemPrompt: z.optional(
    z.enum(["default", "off"], {
      error: `${PLATFORM}.systemPrompt must be "default" or "off"`
    })
  )
});

// ---------------------------------------------------------------------------
// the brief
// ---------------------------------------------------------------------------

/**
 * Wire shape of the submission brief.
 *
 * The two diagnostics report under DIFFERENT paths and always have: an unknown
 * key is `submission.<key> …` (the brief's fields are the caller's
 * `submission.*` fields), while a non-object brief is `submission.submission …`
 * (the brief's own position inside the request envelope). Both are pinned.
 *
 * `null` is admitted wherever the parser treated it as "unset" — that
 * back-compat is the parser's, and the schema states it rather than letting a
 * null reach a normaliser that would have to re-check it.
 */
export const SubmissionSchema = wireObject(
  SUBMISSION,
  {
    model: modelSlugSchema(`${SUBMISSION}.model`),
    system: z.optional(nonEmptyString(`${SUBMISSION}.system`)),
    prompt: PromptSchema,
    assets: SubmissionAssetsSchema,
    mcpServers: z.optional(McpServerRefsSchema),
    secretEnv: z.optional(z.nullable(SecretEnvSchema)),
    environment: z.optional(EnvironmentSchema),
    securityProfile: z.optional(z.nullable(RuntimeSecurityProfileSchema)),
    metadata: z.optional(MetadataSchema),
    fileCapture: z.optional(z.nullable(FileCaptureSchema)),
    builtinTools: z.optional(z.nullable(BuiltinToolsSchema)),
    outputMode: z.optional(z.nullable(OutputModeSchema)),
    responseFormat: z.optional(z.nullable(ResponseFormatSchema)),
    approvalGate: z.optional(z.nullable(ApprovalGateSchema)),
    platform: z.optional(z.nullable(PlatformInjectionSchema))
  },
  { notObject: () => `${SUBMISSION}.submission must be an object` }
);

export type SubmissionWire = z.infer<typeof SubmissionSchema>;
