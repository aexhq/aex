import type { ToolInputSchema } from "./session-config.js";
import { CANONICAL_SHA256_DIGEST_PATTERN } from "./canonical-sha256.js";

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

/** Validate the immutable identity fields common to every submitted resource. */
export function assertPinnedWorkspaceResource(
  value: WorkspaceResourceRef,
  path: string
): void {
  if (typeof value.resourceId !== "string" || !/^wres_[0-9a-f]{32}$/.test(value.resourceId)) {
    throw new Error(`${path}.resourceId must match wres_<32 lowercase hex>`);
  }
  if (!Number.isSafeInteger(value.version) || value.version < 1) {
    throw new Error(`${path}.version must be a positive integer`);
  }
  if (typeof value.assetId !== "string" || value.assetId.length === 0) {
    throw new Error(`${path}.assetId must be a non-empty string`);
  }
  if (!CANONICAL_SHA256_DIGEST_PATTERN.test(value.contentHash)) {
    throw new Error(`${path}.contentHash must be a sha256 digest`);
  }
  const expectedAssetId = `asset_${value.contentHash.slice("sha256:".length)}`;
  if (value.assetId !== expectedAssetId) {
    throw new Error(`${path}.assetId must identify the same bytes as contentHash`);
  }
}
