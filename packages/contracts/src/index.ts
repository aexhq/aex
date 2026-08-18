/**
 * @aex/contracts — brain–hand ABI v1 and session API v1 types, generated from the JSON Schema
 * in contracts/ (see tools/gen.sh). Plus the two hashes both sides of the ABI must agree on.
 */
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

import type { Bounds, LaneRef, StartRequest, ToolManifest } from "./abi.js";

export * as abi from "./abi.js";
export * as session from "./session.js";
export type { paths, components, operations } from "./paths.js";

/** ABI version this package speaks. Major must match between brain and hand. */
export const ABI_MAJOR = 1;
export const ABI_MINOR = 0;

const schema = (name: string): string =>
  readFileSync(new URL(`../schemas/${name}`, import.meta.url), "utf8");

/** Raw JSON Schema documents (parse on demand). */
export const ABI_SCHEMA_JSON: string = schema("abi.v1.json");
export const SESSION_SCHEMA_JSON: string = schema("session.v1.json");
/** The sealed tool manifest v1 and its pinned digest. */
export const TOOL_MANIFEST_V1_JSON: string = schema("tools.manifest.v1.json");
export const TOOL_MANIFEST_V1_DIGEST: string = schema("tools.manifest.v1.digest").trim();

export function toolManifestV1(): ToolManifest {
  return JSON.parse(TOOL_MANIFEST_V1_JSON) as ToolManifest;
}

/**
 * RFC 8785 (JCS) canonical JSON. In JavaScript this is JSON.stringify with object keys sorted by
 * UTF-16 code unit (the default string sort), because JCS defines number and string serialisation
 * by ECMAScript's own JSON.stringify.
 */
export function canonicalize(value: unknown): string {
  const sorted = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(sorted);
    if (v !== null && typeof v === "object") {
      const o = v as Record<string, unknown>;
      return Object.fromEntries(
        Object.keys(o)
          .filter((k) => o[k] !== undefined)
          .sort()
          .map((k) => [k, sorted(o[k])]),
      );
    }
    return v;
  };
  const out = JSON.stringify(sorted(value));
  if (out === undefined) throw new Error("value is not JSON-serialisable");
  return out;
}

/** SHA-256 over the JCS canonical JSON of `value`, lower-case hex. */
export function jcsSha256(value: unknown): string {
  return createHash("sha256").update(canonicalize(value), "utf8").digest("hex");
}

/** Digest of a tool manifest (tools must already be sorted by name). */
export function manifestDigest(manifest: ToolManifest): string {
  return jcsSha256(manifest);
}

/**
 * The `call_hash` of a start request: SHA-256 over JCS of
 * `{tool, input, lane, cwd, detach, bounds}`; absent optional fields serialise as null.
 * Same formula as `aex_contracts::tools::call_hash` in Rust.
 */
export function callHash(req: Pick<StartRequest, "tool" | "input" | "lane" | "cwd" | "detach" | "bounds">): string {
  const identity: { tool: string; input: unknown; lane: LaneRef; cwd: string | null; detach: boolean; bounds: Bounds | null } = {
    tool: req.tool,
    input: req.input,
    lane: req.lane,
    cwd: req.cwd ?? null,
    detach: req.detach,
    bounds: req.bounds ?? null,
  };
  return jcsSha256(identity);
}
