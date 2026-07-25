import type { ToolInputSchema } from "./session-config.js";
import { CANONICAL_SHA256_DIGEST_PATTERN } from "./canonical-sha256.js";
import {
  pinnedWorkspaceResourceSchema,
  workspaceResourceNameSchema
} from "./schemas/workspace-resources.js";
import { parseWire } from "./schemas/wire.js";

/**
 * Persisted workspace file resource names accepted by the public wire boundary.
 *
 * This is deliberately separate from filename validation and from the SDK's
 * filename-to-storage-slug derivation. Equal syntax with instruction names
 * today does not make those resource domains one contract.
 */
export const WORKSPACE_FILE_RESOURCE_NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

/**
 * Persisted workspace instruction resource names accepted by the public wire
 * boundary. Callers supply these names directly; consumers must not normalize,
 * lowercase, trim, or otherwise rewrite an accepted value.
 */
export const WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

/** Assert a directly supplied persisted workspace file resource name. */
export function assertWorkspaceFileResourceName(
  value: unknown,
  path: string
): asserts value is string {
  assertWorkspaceResourceName(value, path, WORKSPACE_FILE_RESOURCE_NAME_PATTERN);
}

/** Assert a directly supplied persisted workspace instruction resource name. */
export function assertWorkspaceInstructionResourceName(
  value: unknown,
  path: string
): asserts value is string {
  assertWorkspaceResourceName(value, path, WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN);
}

function assertWorkspaceResourceName(value: unknown, path: string, pattern: RegExp): asserts value is string {
  parseWire(workspaceResourceNameSchema(path, pattern), value);
}

/** Immutable bytes in the workspace content-addressed asset store. */
export interface AssetIdentity {
  readonly assetId: string;
  /** Canonical `sha256:<hex>` digest of the asset bytes. */
  readonly contentHash: string;
  readonly sizeBytes: number;
  readonly contentType: string;
}

interface WorkspaceResourceRefBase {
  /** Stable logical resource id. */
  readonly resourceId: string;
  /** Immutable positive version of the logical resource. */
  readonly version: number;
  readonly assetId: string;
  readonly contentHash: string;
}

/** A reusable file input pinned to immutable bytes and a workspace version. */
export interface WorkspaceFileRef extends WorkspaceResourceRefBase {
  readonly kind: "file";
  readonly name: string;
  readonly mountPath: string;
}

/** A reusable skill input pinned to immutable bytes and a workspace version. */
export interface WorkspaceSkillRef extends WorkspaceResourceRefBase {
  readonly kind: "skill";
  readonly name: string;
  readonly description: string;
}

/** A reusable custom tool input pinned to immutable bytes and a workspace version. */
export interface WorkspaceToolRef extends WorkspaceResourceRefBase {
  readonly kind: "tool";
  readonly name: string;
  readonly description: string;
  readonly input_schema: ToolInputSchema;
  readonly entry: string;
}

/** Versioned instruction context backed by immutable asset bytes. */
export interface WorkspaceInstructionRef extends WorkspaceResourceRefBase {
  readonly kind: "instruction";
  readonly name: string;
}

export type WorkspaceResourceRef =
  | WorkspaceFileRef
  | WorkspaceSkillRef
  | WorkspaceToolRef
  | WorkspaceInstructionRef;

/** The only resource-bearing section of a session submission. */
export interface SubmissionAssets {
  readonly files: readonly WorkspaceFileRef[];
  readonly skills: readonly WorkspaceSkillRef[];
  readonly tools: readonly WorkspaceToolRef[];
  readonly instructions: readonly WorkspaceInstructionRef[];
}

interface WorkspaceResourceRecordFields {
  readonly createdAt: string;
  readonly updatedAt?: string;
  readonly sizeBytes: number;
  readonly contentType: string;
}

export type WorkspaceFileRecord = WorkspaceFileRef & WorkspaceResourceRecordFields;
export type WorkspaceSkillRecord = WorkspaceSkillRef & WorkspaceResourceRecordFields;
export type WorkspaceToolRecord = WorkspaceToolRef & WorkspaceResourceRecordFields;
export type WorkspaceInstructionRecord = WorkspaceInstructionRef & WorkspaceResourceRecordFields;

export interface WorkspaceResourcePage<T> {
  readonly resources: readonly T[];
  readonly nextCursor?: string;
}

export interface WorkspaceResourceListQuery {
  /** Opaque cursor returned by the previous page. */
  readonly cursor?: string;
  /** Page size. Defaults to 100 and must be an integer from 1 through 100. */
  readonly limit?: number;
}

/**
 * Validate the immutable identity fields common to every submitted resource.
 *
 * The canonical digest grammar is passed to the schema rather than imported by
 * it: `canonical-sha256.ts` declares that grammar once and this module is its
 * declared consumer, which is the arrangement `canonical-sha256-ownership.test.ts`
 * pins.
 */
export function assertPinnedWorkspaceResource(
  value: WorkspaceResourceRef,
  path: string
): void {
  parseWire(pinnedWorkspaceResourceSchema(path, CANONICAL_SHA256_DIGEST_PATTERN), value);
}
