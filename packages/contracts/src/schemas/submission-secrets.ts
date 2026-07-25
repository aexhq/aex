/**
 * Schemas for the `secrets` channel — the half of a submission that is vaulted,
 * excluded from the idempotency hash, and never echoed back.
 *
 * Mounted under {@link InlineSecretsSchema}; nested schemas anchor their message
 * paths on `secrets` for the reason documented in `submission-environment.ts`.
 */
import * as z from "zod/mini";
import {
  PLATFORM_INTERNAL_SECRET_PREFIX,
  SECRET_ENV_NAME_PATTERN
} from "../submission-limits.js";
import { UnknownFieldError } from "../unknown-field-error.js";
import { indexedPath, wireObject, wirePath } from "./wire.js";

const SECRETS = "secrets";
const MCP_SERVERS = `${SECRETS}.mcpServers`;

const McpServerSecretSchema = wireObject(
  // Depth-proof: the bundle is also mounted under the request envelope, and a
  // path folded from the whole reported position would double the segment
  // there. See `indexedPath`.
  indexedPath(MCP_SERVERS),
  {
    name: z.string(),
    url: z.string(),
    headers: z.optional(z.record(z.string(), z.string()))
  },
  {
    unknownKey: (path, key) => `${path}.${key} is not an allowed field; permitted: name, url, headers`
  }
);

/**
 * The array owns the per-entry field messages as well as the duplicate check.
 *
 * Both need the entry's INDEX — `secrets.mcpServers[0].name` — and a check
 * running on the element schema cannot see where in the array it sits: the
 * low-level parse payload carries the value and the issue list, not a path. The
 * element schema keeps what it can state on its own (shape and the allow-list);
 * anything that has to name its position is stated here.
 */
export const McpServerSecretsSchema = z
  .array(McpServerSecretSchema, { error: `${MCP_SERVERS} must be an array` })
  .check(
    z.check((payload) => {
      const entries = payload.value as readonly { readonly name: unknown; readonly url: unknown }[];
      const reject = (message: string): void => {
        payload.issues.push({ code: "custom", message, input: payload.value });
      };
      const seen = new Set<string>();
      for (const [index, entry] of entries.entries()) {
        const base = wirePath(MCP_SERVERS, [index]);
        if (typeof entry.name !== "string" || entry.name.length === 0) {
          reject(`${base}.name must be a non-empty string`);
          return;
        }
        if (typeof entry.url !== "string" || entry.url.length === 0) {
          reject(`${base}.url must be a non-empty string`);
          return;
        }
        if (seen.has(entry.name)) {
          reject(`${MCP_SERVERS} duplicate name: ${entry.name}`);
          return;
        }
        seen.add(entry.name);
      }
    })
  );

/**
 * `envSecrets` as a map of env var name to value.
 *
 * One ordered check for the same reason as `envVars`: the key-name rule and the
 * value rule have distinct messages and the key rule must win.
 */
export const EnvSecretsSchema = z
  .record(z.string(), z.unknown(), { error: `${SECRETS}.envSecrets must be an object` })
  .check(
    z.check((payload) => {
      const reject = (message: string): void => {
        payload.issues.push({ code: "custom", message, input: payload.value });
      };
      for (const [envName, entry] of Object.entries(payload.value as Record<string, unknown>)) {
        if (!SECRET_ENV_NAME_PATTERN.test(envName)) {
          reject(
            `${SECRETS}.envSecrets key "${envName}" must be a valid env var name matching ${SECRET_ENV_NAME_PATTERN.source}`
          );
          return;
        }
        if (typeof entry !== "string" || entry.length === 0) {
          reject(`${SECRETS}.envSecrets.${envName} must be a non-empty string`);
          return;
        }
      }
    })
  );

/**
 * The inline secrets bundle.
 *
 * A key in the platform-internal namespace gets its own message. The generic
 * unknown-field error still wins when it comes first in the caller's object —
 * `unrecognized_keys` reports keys in input order and only the first is named,
 * which is the precedence the parser had and the golden tests pin.
 */
export const InlineSecretsSchema = wireObject(
  SECRETS,
  {
    mcpServers: z.optional(McpServerSecretsSchema),
    envSecrets: z.optional(EnvSecretsSchema)
  },
  {
    // Returns an Error instance, not a string: callers get `objectPath`,
    // `unknownKey` and the ordered `permittedKeys` as structured fields. The
    // reserved-namespace case deliberately returns a plain Error — it is not an
    // unknown field, it is a field the caller may not set.
    unknownKey: (path, key, permitted) =>
      key.startsWith(PLATFORM_INTERNAL_SECRET_PREFIX)
        ? new Error(
            `${path}.${key} uses the platform-internal ${PLATFORM_INTERNAL_SECRET_PREFIX} namespace and may not be set by callers`
          )
        : new UnknownFieldError(path, key, permitted)
  }
);

/** An empty map carries no secrets, so it is dropped rather than landed empty. */
export function normalizeEnvSecrets(
  envSecrets: Record<string, unknown> | undefined
): Readonly<Record<string, string>> | undefined {
  if (envSecrets === undefined || Object.keys(envSecrets).length === 0) {
    return undefined;
  }
  return { ...envSecrets } as Record<string, string>;
}
