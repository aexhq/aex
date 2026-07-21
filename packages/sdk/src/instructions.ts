import { readFile } from "node:fs/promises";
import { strToU8 } from "fflate";
import { bundleSingleFile, hashSkillBundle } from "@aexhq/contracts/internal";

/** Draft instruction context published through `aex.workspace.instructions`. */
export class Instructions {
  readonly #name: string;
  readonly #contentHash: string;
  readonly #zipBytes: Uint8Array;

  private constructor(name: string, contentHash: string, zipBytes: Uint8Array) {
    this.#name = name;
    this.#contentHash = contentHash;
    this.#zipBytes = zipBytes;
  }

  get name(): string {
    return this.#name;
  }

  /** Build a draft whose canonical archive contains one root `AGENTS.md`. */
  static async fromContent(content: string, args: { readonly name: string }): Promise<Instructions> {
    if (typeof content !== "string" || content.length === 0) {
      throw new Error("Instructions.fromContent: content must be a non-empty string");
    }
    if (!args || typeof args.name !== "string" || !WORKSPACE_NAME_RE.test(args.name)) {
      throw new Error(`Instructions.fromContent: name must match ${WORKSPACE_NAME_RE.source}`);
    }
    const bytes = strToU8(content);
    const zip = bundleSingleFile("AGENTS.md", bytes, "Instructions.fromContent");
    return new Instructions(args.name, await hashSkillBundle(zip), zip);
  }

  static async fromPath(path: string, args: { readonly name: string }): Promise<Instructions> {
    return Instructions.fromContent(await readFile(path, "utf8"), args);
  }

  /** @internal */
  _takeDraftBundle(): { readonly name: string; readonly contentHash: string; readonly bytes: Uint8Array } {
    return { name: this.#name, contentHash: this.#contentHash, bytes: this.#zipBytes };
  }

  toJSON(): never {
    throw new Error(
      "Instructions drafts cannot be submitted directly; publish with aex.workspace.instructions.publish(...)"
    );
  }
}

const WORKSPACE_NAME_RE = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
