import { readFile } from "node:fs/promises";
import type { AgentsMdRef } from "@aexhq/contracts";
import { hashSkillBundle } from "./bundle.js";
import { strToU8, zipSync } from "fflate";

/**
 * AgentsMd — a single markdown file delivered to the agent as the
 * first user turn of the session (matches Claude Code's CLAUDE.md
 * behaviour).
 *
 *   const rules = await AgentsMd.fromContent("# Be helpful", { name: "rules" });
 *   await client.submitRun({ agentsMd: [rules], ... });
 *
 * `client.submitRun` materializes the bytes to the hosted asset store before
 * the run lands. Asset deduplication handles repeated uploads automatically.
 */
export class AgentsMd {
  readonly #ref: AgentsMdRef | DraftAgentsMdRef;
  readonly #zipBytes: Uint8Array | undefined;
  #consumed = false;

  constructor(ref: AgentsMdRef | DraftAgentsMdRef, zipBytes?: Uint8Array) {
    this.#ref = ref;
    this.#zipBytes = zipBytes;
  }

  get ref(): AgentsMdRef | DraftAgentsMdRef {
    return this.#ref;
  }

  get isDraft(): boolean {
    return this.#ref.kind === "draft" && !this.#consumed;
  }

  get isConsumed(): boolean {
    return this.#consumed;
  }

  /**
   * Build a draft AgentsMd from a markdown string. The SDK zips the
   * content under the canonical filename `AGENTS.md` so the hash is a
   * pure function of the markdown text.
   */
  static async fromContent(content: string, args: { readonly name: string }): Promise<AgentsMd> {
    if (typeof content !== "string" || content.length === 0) {
      throw new Error("AgentsMd.fromContent: content must be a non-empty string");
    }
    if (!args || typeof args.name !== "string" || !WORKSPACE_NAME_RE.test(args.name)) {
      throw new Error(`AgentsMd.fromContent: name must match ${WORKSPACE_NAME_RE.source}`);
    }
    const zip = zipSync({ "AGENTS.md": [strToU8(content), { mtime: ZIP_EPOCH }] }, { level: 6 });
    const contentHash = await hashSkillBundle(zip);
    const ref: DraftAgentsMdRef = { kind: "draft", name: args.name, contentHash };
    return new AgentsMd(ref, zip);
  }

  /** Node-only convenience: read a markdown file from disk. */
  static async fromPath(path: string, args: { readonly name: string }): Promise<AgentsMd> {
    const content = await readFile(path, "utf8");
    return AgentsMd.fromContent(content, args);
  }

  /**
   * Internal: yield the draft's zipped bytes + metadata so
   * `client.submitRun` can upload it as an asset.
   */
  _takeDraftBundle(): { name: string; contentHash: string; bytes: Uint8Array } | undefined {
    if (this.#consumed) {
      throw new Error(
        "AgentsMd: cannot reuse a consumed AgentsMd in submitRun. Build a fresh one " +
          "via AgentsMd.fromContent(...) / AgentsMd.fromPath(...) per submitRun call."
      );
    }
    if (this.#ref.kind !== "draft" || !this.#zipBytes) {
      return undefined;
    }
    this.#consumed = true;
    return {
      name: this.#ref.name,
      contentHash: this.#ref.contentHash,
      bytes: this.#zipBytes
    };
  }

  toJSON(): AgentsMdRef {
    if (this.#ref.kind === "draft") {
      throw new Error(
        "AgentsMd: draft AgentsMd cannot be JSON-serialised — it only becomes a wire " +
          "ref when client.submitRun uploads the bytes as an asset."
      );
    }
    return this.#ref;
  }
}

export interface DraftAgentsMdRef {
  readonly kind: "draft";
  readonly name: string;
  readonly contentHash: string;
}

const WORKSPACE_NAME_RE = /^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$/;
const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));
