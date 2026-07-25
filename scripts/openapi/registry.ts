/**
 * What the generated OpenAPI documents describe, and under what component names.
 *
 * A build-time module. It imports **full `zod`** to build the registry (the mini
 * entrypoint tree-shakes the JSON Schema converter out) and the schemas
 * themselves from `@aexhq/contracts` source — the same objects the server
 * validates with, so there is nothing here to keep in sync with the wire.
 *
 * `.meta({ id })` is mandatory for every reused schema: without an explicit id,
 * `reused:"ref"` invents `__schema0`, `__schema1` (measured), which are
 * unreadable and unstable across unrelated edits.
 *
 * **Known gap, stated rather than hidden:** the members of the largest request
 * objects are still `unspecifiedField` while their field validation lives in the
 * parsers. Those objects therefore generate with untyped properties. The
 * generated document is honest about the KEYS a request may carry and silent
 * about their value shapes — a weaker-but-true document, per the plan's rule.
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

/** The registry the generator converts in one pass. */
export const OPENAPI_SCHEMA_REGISTRY = z.globalRegistry;

/**
 * Operation id -> request body component name.
 *
 * Only operations whose request shape is already expressed as a schema appear
 * here. An operation missing from this map generates without a request body,
 * which is a stated gap rather than a claim that it takes none — see the
 * coverage note the generator prints.
 */
export const OPENAPI_REQUEST_BODIES: Readonly<Record<string, string>> = {};
