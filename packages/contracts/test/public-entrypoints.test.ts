import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import * as internalContracts from "../src/internal.js";
import * as publicContracts from "../src/index.js";

// These imports deliberately fail if an internal contract leaks back onto the
// customer entrypoint. One directive per symbol makes each exclusion explicit.
// @ts-expect-error Platform request envelopes are internal transport inputs.
import type { PlatformSessionSubmissionRequest } from "../src/index.js";
// @ts-expect-error Machine placement is an internal submission concern.
import type { SessionMachine } from "../src/index.js";
// @ts-expect-error Connection tickets are coordinator authentication details.
import type { ConnectionTicketChannel } from "../src/index.js";
// @ts-expect-error Runtime security profiles are platform policy details.
import type { RuntimeSecurityProfile } from "../src/index.js";
// @ts-expect-error Custody manifests are platform bookkeeping.
import type { CustodyManifestV1 } from "../src/index.js";
// @ts-expect-error Retention writers are platform bookkeeping.
import type { SessionRetentionPolicyV1 } from "../src/index.js";
// @ts-expect-error Side-effect audit records are platform bookkeeping.
import type { SideEffectAuditEventV1 } from "../src/index.js";
// @ts-expect-error Cleanup progress is not a public session lifecycle state.
import type { CleanupStatus } from "../src/index.js";
// @ts-expect-error SessionUnit was an orphan implementation snapshot and is removed.
import type { SessionUnit } from "../src/index.js";
// @ts-expect-error Persisted workspace secret values are write-only through the public API.
import type { SecretReveal } from "../src/index.js";
// @ts-expect-error Subagent nested-input admission is private runner infrastructure.
import { parseSubagentAssetsInput } from "../src/index.js";

type InternalSurface =
  | PlatformSessionSubmissionRequest
  | SessionMachine
  | ConnectionTicketChannel
  | RuntimeSecurityProfile
  | CustodyManifestV1
  | SessionRetentionPolicyV1
  | SideEffectAuditEventV1
  | CleanupStatus
  | SessionUnit
  | SecretReveal;
void (undefined as unknown as InternalSurface);
void parseSubagentAssetsInput;

const removedRuntimeExports = [
  "CLEANUP_STATUSES",
  "CUSTODY_MANIFEST_SCHEMA_VERSION",
  "bundleSingleFile",
  "deriveSkillName",
  "extractSkillFrontmatter",
  "hashSkillBundle",
  "RUNTIME_SECURITY_PROFILES",
  "SESSION_RETENTION_SCHEMA_VERSION",
  "SIDE_EFFECT_AUDIT_SCHEMA_VERSION",
  "mintConnectionTicket",
  "normalizeToolManifest",
  "operations",
  "parseSessionSubmissionRequest",
  "sha256",
  "stableStringify",
  "verifyConnectionTicket"
] as const;

describe("contracts entrypoint boundary", () => {
  it("publishes only the customer root and explicit workspace-internal subpaths", () => {
    const packageJson = JSON.parse(
      readFileSync(new URL("../package.json", import.meta.url), "utf8")
    ) as { readonly exports?: Readonly<Record<string, unknown>> };
    expect(Object.keys(packageJson.exports ?? {})).toEqual([".", "./internal", "./subagent-runtime"]);
  });

  it("keeps platform implementation values off the customer entrypoint", () => {
    for (const name of removedRuntimeExports) {
      expect(publicContracts, name).not.toHaveProperty(name);
    }
  });

  it("retains platform contracts behind the explicit internal entrypoint", () => {
    expect(internalContracts.parseSessionSubmissionRequest).toBeTypeOf("function");
    expect(internalContracts.mintConnectionTicket).toBeTypeOf("function");
    expect(internalContracts.operations.getSession).toBeTypeOf("function");
    expect(internalContracts.CLEANUP_STATUSES).toContain("running");
    expect(internalContracts.SESSION_RETENTION_SCHEMA_VERSION).toBe(1);
    expect(internalContracts.CUSTODY_MANIFEST_SCHEMA_VERSION).toBe(1);
    expect(internalContracts.bundleSkillFiles).toBeTypeOf("function");
    expect(internalContracts.hashSkillBundle).toBeTypeOf("function");
    expect(internalContracts.normalizeToolManifest).toBeTypeOf("function");
  });
});
