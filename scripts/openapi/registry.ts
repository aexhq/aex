/**
 * What the generated OpenAPI documents describe, and under what component names.
 *
 * A build-time module. It imports **full `zod`** to build the registry (the mini
 * entrypoint tree-shakes the JSON Schema converter out) and the schemas
 * themselves from `@aexhq/contracts` source — the same objects the server
 * validates with, so there is nothing here to keep in sync with the wire.
 *
 * An explicit id is mandatory for every reused schema: without one,
 * `reused:"ref"` invents `__schema0`, `__schema1` (measured), which are
 * unreadable and unstable across unrelated edits. Ids are attached with
 * `.register(z.globalRegistry, …)` — **`.meta()` does not exist on `zod/mini`
 * schemas**, and the two entrypoints share one global registry object, so a
 * mini schema's metadata is visible to this full-zod converter.
 *
 * **What the document still understates.** Some inferred field types are
 * deliberately weaker-but-true — `prompt`, `mcpServers`, `secretEnv`,
 * `builtinTools` and the capture lists validate through ordered `z.check()`
 * ladders whose per-entry wording a typed union would destroy. Each carries a
 * registered description naming the rule and its owning module. A
 * weaker-but-true schema beats a precise-but-wrong one.
 */
import { z } from "zod";
import {
  EnvironmentSchema,
  EnvVarsSchema,
  NetworkingSchema,
  PackagesSchema,
  PlatformPackageSchema
} from "../../packages/contracts/src/schemas/submission-environment.js";
import {
  EnvSecretsSchema,
  InlineSecretsSchema,
  McpServerSecretsSchema
} from "../../packages/contracts/src/schemas/submission-secrets.js";
import { SessionWebhookSchema } from "../../packages/contracts/src/schemas/session-webhook.js";
import { SessionLimitsSchema } from "../../packages/contracts/src/schemas/session-limits.js";
import { SessionMachineSchema } from "../../packages/contracts/src/schemas/session-machine.js";
import { SessionSubmissionRequestSchema } from "../../packages/contracts/src/schemas/submission-request.js";
import {
  ApprovalGateSchema,
  FileCaptureSchema,
  PlatformInjectionSchema,
  ResponseFormatSchema,
  SubmissionSchema
} from "../../packages/contracts/src/schemas/submission-body.js";
import { SubmissionAssetsSchema } from "../../packages/contracts/src/schemas/submission-assets.js";

/**
 * Cross-field and bounds rules expressed as `.check()` are SILENTLY ABSENT from
 * generated JSON Schema (measured). Every schema that carries one states the
 * rule and its owning module here, so a spec consumer is told what the server
 * enforces even though the document cannot express it.
 */
const RULE_DESCRIPTIONS: Readonly<Record<string, string>> = {
  SessionWebhook:
    "Run-callback registration. `url` must be https with no userinfo — enforced in " +
    "packages/contracts/src/schemas/session-webhook.ts, and not expressible in JSON Schema.",
  SessionLimits:
    "Per-session lineage-limit override. Shape and positivity only; clamping to the workspace and " +
    "platform ceilings happens server-side in resolveSessionLimits.",
  SessionMachine:
    "Capacity intent. An object with no `spot` carries no signal and is dropped; `spot: false` is " +
    "preserved as an explicit request for standard capacity.",
  SubmissionNetworking:
    "Egress policy. `mode` is required whenever `networking` is supplied — enforced in " +
    "packages/contracts/src/schemas/submission-environment.ts.",
  SubmissionAllowedHosts:
    "Allowed egress hosts. Compared case-insensitively; duplicates are rejected.",
  SubmissionPackage:
    'Package request. `name` may carry an ecosystem prefix ("pip:pandas"); an unprefixed name ' +
    "defaults to apt and an unknown prefix is rejected.",
  SubmissionEnvironment: "Customer-controlled runtime environment.",
  SecretsMcpServers: "Per-session MCP server credentials. Server names must be unique.",
  SecretsEnvSecrets:
    "Per-session env-var secret values. Keys must be valid env var names; each pairs with a " +
    "`submission.secretEnv` declaration.",
  InlineSecrets:
    "The vaulted half of a submission. Excluded from the idempotency hash and never echoed back."
};

function register<Schema extends object>(id: string, schema: Schema): Schema {
  z.globalRegistry.add(schema as never, {
    id,
    ...(RULE_DESCRIPTIONS[id] ? { description: RULE_DESCRIPTIONS[id] } : {})
  });
  return schema;
}

register("SessionWebhook", SessionWebhookSchema);
register("SessionLimits", SessionLimitsSchema);
register("SessionMachine", SessionMachineSchema);
register("SubmissionEnvironment", EnvironmentSchema);
register("SubmissionNetworking", NetworkingSchema);
register("SubmissionPackages", PackagesSchema);
register("SubmissionPackage", PlatformPackageSchema);
register("SubmissionEnvVars", EnvVarsSchema);
register("InlineSecrets", InlineSecretsSchema);
register("SecretsMcpServers", McpServerSecretsSchema);
register("SecretsEnvSecrets", EnvSecretsSchema);
register("SessionSubmissionRequest", SessionSubmissionRequestSchema);
register("Submission", SubmissionSchema);
register("SubmissionAssets", SubmissionAssetsSchema);
register("SubmissionFileCapture", FileCaptureSchema);
register("SubmissionResponseFormat", ResponseFormatSchema);
register("SubmissionApprovalGate", ApprovalGateSchema);
register("SubmissionPlatformInjection", PlatformInjectionSchema);

/** The registry the generator converts in one pass. */
export const OPENAPI_SCHEMA_REGISTRY = z.globalRegistry;

/**
 * Operation id -> request body component name.
 *
 * Only operations whose request shape is expressed as a schema appear here. An
 * operation missing from this map generates without a request body, which the
 * document states as a gap rather than as a claim that the route takes no body.
 *
 * `sessions.create` is the whole submission envelope, and the one that matters
 * most — it is the largest request surface in the API. The remaining POST
 * routes (`secrets.create`, `mcpServers.create`, `workspace.*.publish`,
 * `billing.*`, `adminBilling.*`) take bodies that are validated server-side and
 * have no schema in this package yet; binding a placeholder would assert a shape
 * nobody checks.
 */
export const OPENAPI_REQUEST_BODIES: Readonly<Record<string, string>> = {
  "sessions.create": "SessionSubmissionRequest"
};
