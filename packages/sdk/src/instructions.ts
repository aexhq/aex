import { readFile } from "node:fs/promises";
import {
  assertWorkspaceInstructionResourceName,
  hashWorkspaceInstructionText,
  normalizeWorkspaceInstructionText
} from "@aexhq/contracts";

/**
 * Draft instruction context published through `aex.workspace.instructions`.
 *
 * The draft holds TEXT. It used to hold a canonical single-entry ZIP wrapping
 * `AGENTS.md`, which existed only so the control plane could unzip it back out
 * on every submit — see
 * references/modular-open-source-2026-07-27/11-archive-registration-redesign.md.
 * With the archive gone there is nothing to upload and nothing to content-address:
 * the pin is `textHash`, a digest of the trimmed text itself.
 */
export class Instructions {
  readonly #name: string;
  readonly #text: string;
  readonly #textHash: string;

  private constructor(name: string, text: string, textHash: string) {
    this.#name = name;
    this.#text = text;
    this.#textHash = textHash;
  }

  get name(): string {
    return this.#name;
  }

  /** Build a draft carrying the trimmed instruction text and its digest. */
  static async fromContent(content: string, args: { readonly name: string }): Promise<Instructions> {
    if (typeof content !== "string" || content.length === 0) {
      throw new Error("Instructions.fromContent: content must be a non-empty string");
    }
    const name = args?.name;
    assertWorkspaceInstructionResourceName(name, "Instructions.fromContent: name");
    const text = normalizeWorkspaceInstructionText(content, "Instructions.fromContent: content");
    return new Instructions(name, text, await hashWorkspaceInstructionText(text));
  }

  static async fromPath(path: string, args: { readonly name: string }): Promise<Instructions> {
    return Instructions.fromContent(await readFile(path, "utf8"), args);
  }

  /** @internal */
  _takeDraftInstruction(): { readonly name: string; readonly text: string; readonly textHash: string } {
    return { name: this.#name, text: this.#text, textHash: this.#textHash };
  }

  toJSON(): never {
    throw new Error(
      "Instructions drafts cannot be submitted directly; publish with aex.workspace.instructions.publish(...)"
    );
  }
}
