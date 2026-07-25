/**
 * Schemas for `submission.environment` — the customer-controlled runtime
 * environment: egress policy, OS/language packages, and plain env vars.
 *
 * Two behaviours here are deliberately NOT in the schemas, per D4/L1:
 *
 * - **Ecosystem-prefix splitting** (`"pip:pandas"` -> `{name:"pandas",
 *   ecosystem:"pip"}`) is a transform, so it lives in
 *   {@link normalizePlatformPackage}. A `.transform()` would make the response
 *   half of the generated spec ungenerable.
 * - **Host lower-casing** lives in {@link normalizeAllowedHosts} for the same
 *   reason.
 */
import * as z from "zod/mini";
import {
  ENV_VARS_MAX_ENTRIES,
  ENV_VARS_MAX_TOTAL_BYTES,
  ENV_VARS_MAX_VALUE_BYTES,
  ENV_VAR_KEY_PATTERN,
  AEX_RESERVED_ENV_PREFIX,
  PLATFORM_PACKAGE_ECOSYSTEMS
} from "../submission-limits.js";
import { indexedPath, lastSegment, wireObject } from "./wire.js";

/**
 * Every schema below is mounted under {@link EnvironmentSchema}, which is itself
 * mounted under the submission brief. That matters for paths: Zod reports an
 * issue position relative to the schema the parse STARTED at, so a message
 * rendered by folding the whole reported position is only correct at one mount
 * depth. Every path below is either a fixed string or derived from the last
 * segment(s) Zod reports (`indexedPath`, `lastSegment`), which is what keeps it
 * right however deeply the environment is mounted.
 */
const ENVIRONMENT = "submission.environment";
const NETWORKING = `${ENVIRONMENT}.networking`;
const PACKAGES = `${ENVIRONMENT}.packages`;
const ENV_VARS = `${ENVIRONMENT}.envVars`;

/**
 * `envVars` as a bounded map of shell-portable names to UTF-8 values.
 *
 * The whole rule set is one ordered `z.check()` rather than a composition of
 * record key/value schemas, because the ladder is order-sensitive and every rung
 * has its own message: a key that is both malformed AND non-string must report
 * the malformed key, and the byte budget must fail on the entry that crosses
 * `ENV_VARS_MAX_TOTAL_BYTES` — not on the map as a whole. Composed schemas
 * report in Zod's order, not this one.
 *
 * The cost is that these rules are invisible in the generated OpenAPI document
 * (L2), so the description below is what a spec consumer sees. A weaker-but-true
 * schema beats a precise-but-wrong one.
 */
export const EnvVarsSchema = z
  .record(z.string(), z.unknown(), { error: `${ENV_VARS} must be an object` })
  .check(
    z.check((payload) => {
      const value = payload.value as Record<string, unknown>;
      const keys = Object.keys(value);
      const reject = (message: string): void => {
        payload.issues.push({ code: "custom", message, input: payload.value });
      };

      if (keys.length > ENV_VARS_MAX_ENTRIES) {
        reject(`${ENV_VARS} has ${keys.length} entries; maximum is ${ENV_VARS_MAX_ENTRIES}`);
        return;
      }

      let totalBytes = 0;
      for (const key of keys) {
        if (!ENV_VAR_KEY_PATTERN.test(key)) {
          reject(`${ENV_VARS}.${key} key must match /^[A-Z_][A-Z0-9_]*$/`);
          return;
        }
        if (key.startsWith(AEX_RESERVED_ENV_PREFIX)) {
          reject(
            `${ENV_VARS}.${key} uses reserved prefix "${AEX_RESERVED_ENV_PREFIX}" (set by the aex runtime)`
          );
          return;
        }
        const raw = value[key];
        if (typeof raw !== "string") {
          reject(`${ENV_VARS}.${key} must be a string`);
          return;
        }
        if (raw.includes("\0")) {
          reject(`${ENV_VARS}.${key} must not contain NUL bytes`);
          return;
        }
        const valueBytes = Buffer.byteLength(raw, "utf8");
        if (valueBytes > ENV_VARS_MAX_VALUE_BYTES) {
          reject(
            `${ENV_VARS}.${key} value is ${valueBytes} bytes; maximum is ${ENV_VARS_MAX_VALUE_BYTES}`
          );
          return;
        }
        totalBytes += Buffer.byteLength(key, "utf8") + valueBytes;
        if (totalBytes > ENV_VARS_MAX_TOTAL_BYTES) {
          reject(`${ENV_VARS} total byte size exceeds maximum ${ENV_VARS_MAX_TOTAL_BYTES}`);
          return;
        }
      }
    })
  )
  .register(z.globalRegistry, {
    id: "SubmissionEnvVars",
    description:
      "Environment variables injected into the session runtime. Keys match /^[A-Z_][A-Z0-9_]*$/ " +
      `and must not use the reserved "${AEX_RESERVED_ENV_PREFIX}" prefix. Bounded to ` +
      `${ENV_VARS_MAX_ENTRIES} entries, ${ENV_VARS_MAX_VALUE_BYTES} bytes per value and ` +
      `${ENV_VARS_MAX_TOTAL_BYTES} bytes in total. Enforced in packages/contracts/src/schemas/submission-environment.ts.`
  });

/** A single allowed egress host. Case folding is {@link normalizeAllowedHosts}' job. */
const allowedHost = z.string().check(
  z.refine((value: string) => typeof value === "string" && value.length > 0, {
    error: (issue) =>
      `${NETWORKING}.allowedHosts[${lastSegment(issue.path)}] must be a non-empty string`,
    abort: true
  })
);

export const AllowedHostsSchema = z
  .array(allowedHost, {
    error: `${NETWORKING}.allowedHosts must be an array of strings`
  })
  .check(
    z.check((payload) => {
      const seen = new Set<string>();
      for (const entry of payload.value as readonly string[]) {
        const lower = entry.toLowerCase();
        if (seen.has(lower)) {
          payload.issues.push({
            code: "custom",
            message: `${NETWORKING}.allowedHosts duplicate entry: ${entry}`,
            input: payload.value
          });
          return;
        }
        seen.add(lower);
      }
    })
  );

/** Lower-case every host so the deny/allow comparison downstream is case-insensitive. */
export function normalizeAllowedHosts(hosts: readonly string[]): readonly string[] {
  return hosts.map((host) => host.toLowerCase());
}

export const NetworkingSchema = wireObject(
  NETWORKING,
  {
    mode: z.optional(
      z.enum(["limited", "open"], {
        error: `${NETWORKING}.mode must be one of: limited, open`
      })
    ),
    allowedHosts: z.optional(AllowedHostsSchema)
  },
  {
    unknownKey: (path, key) =>
      `${path}.${key} is not an allowed field; permitted: mode, allowedHosts`
  }
).check(
  // `mode` is optional on the wire only so that an unknown-key error outranks a
  // missing-mode error, matching the parser's order. Supplying `networking` at
  // all is a commitment to state the egress mode.
  z.refine(
    (value: { readonly mode?: "limited" | "open" | undefined }) => value.mode !== undefined,
    {
      error: `${NETWORKING}.mode is required when networking is provided`,
      abort: true
    }
  )
);

export const PlatformPackageSchema = wireObject(
  indexedPath(PACKAGES),
  {
    name: z.string(),
    version: z.optional(z.string())
  },
  {
    unknownKey: (path, key) => `${path}.${key} is not an allowed field; permitted: name, version`
  }
);

export const PackagesSchema = z.array(PlatformPackageSchema, {
  error: `${PACKAGES} must be an array`
});

/**
 * Split the `"<ecosystem>:<name>"` prefix a caller encodes into `name`.
 *
 * An unprefixed name defaults to `apt`. A colon-delimited prefix that is not a
 * known ecosystem is rejected rather than folded into the package name, so a
 * typo'd manager fails closed instead of installing something unexpected.
 *
 * Throws rather than returning a result because it runs inside the parser's
 * existing `withContractParseError` boundary, which brands whatever it throws.
 */
export function normalizePlatformPackage(
  pkg: { readonly name: string; readonly version?: string | undefined },
  path: string
): { readonly name: string; readonly version?: string; readonly ecosystem: string } {
  let ecosystem = "apt";
  let name = pkg.name;
  const colon = pkg.name.indexOf(":");
  if (colon > 0) {
    const prefix = pkg.name.slice(0, colon);
    if (!(PLATFORM_PACKAGE_ECOSYSTEMS as readonly string[]).includes(prefix)) {
      throw new Error(
        `${path}.name has unknown ecosystem prefix "${prefix}:"; permitted: ${PLATFORM_PACKAGE_ECOSYSTEMS.join(", ")}`
      );
    }
    ecosystem = prefix;
    name = pkg.name.slice(colon + 1);
  }
  if (name.length === 0) {
    throw new Error(
      `${path}.name resolves to an empty package after stripping the "${ecosystem}:" ecosystem prefix`
    );
  }
  return pkg.version !== undefined ? { name, version: pkg.version, ecosystem } : { name, ecosystem };
}

export const EnvironmentSchema = wireObject(
  ENVIRONMENT,
  {
    networking: z.optional(NetworkingSchema),
    packages: z.optional(PackagesSchema),
    envVars: z.optional(EnvVarsSchema)
  },
  {
    unknownKey: (path, key) =>
      `${path}.${key} is not an allowed field; permitted: networking, packages, envVars`
  }
);
