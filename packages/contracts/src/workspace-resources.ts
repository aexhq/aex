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
 * Maximum UTF-8 byte length of ONE published workspace instruction document.
 *
 * Instruction text is not an archive and deliberately does not inherit an
 * archive cap. Those caps bound a DEFLATE inflate, they are measured in
 * mebibytes, and they are being removed; neither their magnitude nor their
 * reason transfers to a system-prompt document.
 *
 * VALUE (128,000 bytes): the same arithmetic the inline free-text admission cap
 * uses (`SESSION_MAX_INPUT_TEXT_BYTES`, 512,000 bytes = the whole of the
 * smallest served 128,000-token context window at the standard ~4-bytes/token
 * heuristic), taken at a QUARTER of that window — about 32,000 tokens. An
 * instruction is not in-band free text: it is re-injected into the system
 * prompt on every turn of the session, and it sits alongside the platform
 * prompt, the tool schemas, the customer's own prompt (bounded separately at
 * 512,000 bytes) and the completion. A quarter of the smallest window is the
 * loosest per-document bound under which one instruction cannot, by itself,
 * crowd all four of those out. As with the inline cap this is a CORRECTNESS
 * gate, not a product cap: text over it produces a session that can only fail
 * at the provider, after the customer has been billed for the boot.
 *
 * Declared here, once, and imported by every enforcement site. The archive caps
 * this replaces were duplicated by hand across the publication and submit
 * paths, drifted 4x apart, and the drift survived because the copy carried a
 * comment asserting they agreed.
 */
export const WORKSPACE_INSTRUCTION_MAX_TEXT_BYTES = 128_000;

/**
 * Canonicalise instruction text: trimmed, non-empty, within the byte bound.
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
  const bytes = utf8ByteLength(text);
  if (bytes > WORKSPACE_INSTRUCTION_MAX_TEXT_BYTES) {
    throw new Error(
      `${path} exceeds the ${WORKSPACE_INSTRUCTION_MAX_TEXT_BYTES}-byte instruction limit (got ${bytes})`
    );
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
