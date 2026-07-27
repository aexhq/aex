import type { ToolInputSchema } from "./session-config.js";
import { CANONICAL_SHA256_DIGEST_PATTERN } from "./canonical-sha256.js";
import {
  pinnedWorkspaceInstructionSchema,
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

/**
 * Canonicalise instruction text: trimmed and non-empty. There is NO byte bound.
 *
 * WHY NO BOUND. The first cut of this contract carried
 * `WORKSPACE_INSTRUCTION_MAX_TEXT_BYTES = 128_000`, a hard rejection at
 * authoring time derived as a quarter of the smallest served context window.
 * That was OUR limit, not a real constraint — the same mistake as the archive
 * compressed cap this workstream exists to remove. Two things were wrong with
 * it:
 *
 *   1. It was a PROMPT budget enforced at a point that cannot know the prompt.
 *      A context window is a per-SESSION fact of the model a submission names;
 *      publication happens before any submission exists, so the bound could
 *      only be the smallest window and it charged every customer for the
 *      weakest model they might never use.
 *   2. A model limit is worked around, not refused. Text that does not fit the
 *      session's inline budget is STAGED to a workspace file at compose time
 *      and pointed at from the system prompt (see the platform's
 *      `instructionInlineBudgetBytes` / `platform-runtime-agent` manifest
 *      builder). Refusing publication removes the customer's only path to a
 *      large instruction; staging keeps it.
 *
 * The only bound that survives is a STORAGE/TRANSPORT one, and it is imposed by
 * infrastructure rather than declared here: the publish request body itself is
 * capped by API Gateway (10 MB) and the Lambda synchronous payload limit
 * (6 MB), which reject an oversized document at the edge with their own error
 * before any of this code runs. That is a real constraint about moving bytes,
 * stated as such — not a prompt budget wearing a storage name.
 *
 * Trimming is part of the contract rather than a courtesy — the stored text,
 * its `textHash`, and its `sizeBytes` all describe the trimmed form, so a
 * caller that pads their file cannot mint a second immutable version of the
 * same document.
 */
export function normalizeWorkspaceInstructionText(value: unknown, path: string): string {
  if (typeof value !== "string") {
    throw new Error(`${path} must be a non-empty string`);
  }
  const text = value.trim();
  if (text.length === 0) {
    throw new Error(`${path} must be non-empty after trimming`);
  }
  return text;
}

/** Assert instruction text without taking the canonical form. */
export function assertWorkspaceInstructionText(
  value: unknown,
  path: string
): asserts value is string {
  normalizeWorkspaceInstructionText(value, path);
}

/** UTF-8 byte length of the canonical (trimmed) instruction text. */
export function workspaceInstructionTextBytes(text: string): number {
  return utf8ByteLength(text);
}

/**
 * The immutability pin for an instruction: `sha256:<hex>` over the UTF-8 bytes
 * of the TRIMMED text.
 *
 * Web-Crypto only, for the same reason `hashSkillBundle` is: one
 * implementation has to serve the SDK, the CLI, and the hosted control plane,
 * and only Web Crypto is present in all three.
 */
export async function hashWorkspaceInstructionText(text: string): Promise<string> {
  const subtle = (globalThis as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (!subtle) {
    throw new Error(
      "hashWorkspaceInstructionText: globalThis.crypto.subtle is not available; " +
        "Bun, Node 18+, or a Web-Crypto-capable runtime is required"
    );
  }
  const bytes = new TextEncoder().encode(text.trim());
  const digest = await subtle.digest("SHA-256", bytes.buffer as ArrayBuffer);
  const view = new Uint8Array(digest);
  let hex = "";
  for (let index = 0; index < view.length; index += 1) {
    hex += (view[index] as number).toString(16).padStart(2, "0");
  }
  return `sha256:${hex}`;
}

function utf8ByteLength(text: string): number {
  return new TextEncoder().encode(text).byteLength;
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

/**
 * Versioned instruction context pinned to immutable TEXT.
 *
 * Deliberately NOT a {@link WorkspaceResourceRefBase}: instruction text is not
 * a content-addressed object. It has no identity separate from itself, so it
 * carries a hash of the text instead of an `assetId`/`contentHash` pair naming
 * bytes in the asset store.
 */
export interface WorkspaceInstructionRef {
  readonly kind: "instruction";
  /** Stable logical resource id. */
  readonly resourceId: string;
  /** Immutable positive version of the logical resource. */
  readonly version: number;
  readonly name: string;
  /** Canonical `sha256:<hex>` digest of the trimmed UTF-8 instruction text. */
  readonly textHash: string;
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

/**
 * An instruction record carries no `contentType`: there is no asset to type.
 * `sizeBytes` survives and now means the UTF-8 byte length of the trimmed text
 * rather than the compressed size of an archive.
 */
export type WorkspaceInstructionRecord = WorkspaceInstructionRef & {
  readonly createdAt: string;
  readonly updatedAt?: string;
  readonly sizeBytes: number;
};

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
 * Validate the immutable identity fields of a submitted resource.
 *
 * Two shapes, because an instruction is pinned to text and the other three to
 * bytes. Asset-backed kinds carry the `assetId`/`contentHash` quartet;
 * `instruction` carries `textHash` alone.
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
  if (value.kind === "instruction") {
    parseWire(pinnedWorkspaceInstructionSchema(path, CANONICAL_SHA256_DIGEST_PATTERN), value);
    return;
  }
  parseWire(pinnedWorkspaceResourceSchema(path, CANONICAL_SHA256_DIGEST_PATTERN), value);
}
