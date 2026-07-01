import { SECRET_ENV_NAME_PATTERN, SECRET_HANDLE_PATTERN, SecretString } from "@aexhq/contracts";

/**
 * Non-secret wire shape for one `secretEnv` entry. A workspace ref carries the
 * handle (hashable, resolved server-side); an ephemeral secret carries only a
 * value-free placeholder — the value is split into the vaulted secrets channel
 * and excluded from the idempotency hash.
 */
export type SecretEnvSubmissionEntry = { readonly ref: string } | { readonly ephemeral: true };

/**
 * Minimal client surface `secret.upload` needs to promote an ephemeral secret
 * into the workspace store. `AgentExecutor` satisfies it via its `secrets`
 * client; defined structurally here so `secret.ts` does not import `client.ts`
 * (which would be circular — `client.ts` imports `Secret`). Mirrors
 * {@link SkillUploader}.
 */
export interface SecretUploader {
  _createWorkspaceSecret(args: { readonly name: string; readonly value: string }): Promise<{ readonly name: string }>;
}

/**
 * A secret with the SAME lifecycle semantic as `Skill` / `File` / `AgentsMd`:
 * EPHEMERAL per-run by default, PROMOTABLE to a persisted, name-searchable
 * workspace secret you can reference and reuse.
 *
 *   - `Secret.value(v)` — EPHEMERAL per-run: the value is vaulted when the session is created and
 *     excluded from the idempotency hash; only a `{ ephemeral: true }`
 *     placeholder rides the (hashed) submission. Deleted when the run finishes.
 *     Clean, no workspace dependency. ≙ `Skill.fromFiles(...)` (a draft).
 *   - `secret.upload(client, { name })` — PROMOTE that value into the workspace
 *     secret store under `name`; resolves to a `Secret.ref`. ≙ `skill.upload(client)`.
 *   - `Secret.ref(handle)` — WORKSPACE: only the handle rides the submission;
 *     the value is resolved server-side from the workspace secret store. No
 *     value ever travels. ≙ `Skill.fromCatalog(record)`.
 *
 * The SDK splits each `Secret` BEFORE the wire payload is built (exactly how
 * `McpServer` splits `headers` into `secrets.mcpServers`): the env-var name keys
 * `submission.secretEnv[name] = toSubmissionEntry()`, and for ephemeral values
 * `secrets.envSecrets[name] = toSecretValue()`.
 */
export class Secret {
  readonly kind: "value" | "ref";
  /** Workspace handle (ref kind only); `undefined` for ephemeral values. */
  readonly handle: string | undefined;
  readonly #value: SecretString | undefined;
  /** True once promoted via `upload` — a consumed ephemeral can't be reused. */
  #consumed = false;

  /** Internal constructor. Use `Secret.value(...)` or `Secret.ref(...)`. */
  constructor(args: { readonly kind: "value"; readonly value: SecretString } | { readonly kind: "ref"; readonly handle: string }) {
    if (!args || typeof args !== "object") {
      throw new Error("Secret: args is required");
    }
    if (args.kind === "ref") {
      if (typeof args.handle !== "string" || !SECRET_HANDLE_PATTERN.test(args.handle)) {
        throw new Error(`Secret.ref: handle must match ${SECRET_HANDLE_PATTERN.source}`);
      }
      this.kind = "ref";
      this.handle = args.handle;
      this.#value = undefined;
      return;
    }
    this.kind = "value";
    this.handle = undefined;
    this.#value = args.value;
  }

  /** Ephemeral per-run value. Vaulted when the session is created; never in the spec/hash; gone at terminal. */
  static value(value: string | SecretString): Secret {
    const wrapped = value instanceof SecretString ? value : new SecretString(value, "secret");
    if (!wrapped.unwrap()) {
      throw new Error("Secret.value: value must be a non-empty string");
    }
    return new Secret({ kind: "value", value: wrapped });
  }

  /** Reference a workspace secret by handle; resolved server-side when the session is created. */
  static ref(handle: string): Secret {
    return new Secret({ kind: "ref", handle });
  }

  /** True once this ephemeral secret has been promoted via `upload`. */
  get isConsumed(): boolean {
    return this.#consumed;
  }

  /**
   * Promote this EPHEMERAL secret into the workspace secret store under `name`
   * and return a `Secret.ref(name)` for reuse across runs. Blocking: the store
   * write completes before this resolves. Consumes this instance (an ephemeral
   * value is promoted exactly once), mirroring `Skill.upload`.
   *
   * Only valid on a `Secret.value(...)`; a `Secret.ref(...)` is already
   * persisted.
   */
  async upload(client: SecretUploader, args: { readonly name: string }): Promise<Secret> {
    if (this.kind !== "value") {
      throw new Error("Secret.upload: only ephemeral Secret.value(...) secrets can be uploaded; a Secret.ref is already persisted");
    }
    if (this.#consumed) {
      throw new Error("Secret.upload: this ephemeral secret was already consumed. Build a fresh Secret.value(...) to re-upload.");
    }
    if (!args || typeof args.name !== "string" || !SECRET_HANDLE_PATTERN.test(args.name)) {
      throw new Error(`Secret.upload: name must match ${SECRET_HANDLE_PATTERN.source}`);
    }
    const value = this.#value!.unwrap();
    await client._createWorkspaceSecret({ name: args.name, value });
    this.#consumed = true;
    return Secret.ref(args.name);
  }

  /** Non-secret wire entry for `submission.secretEnv[<envName>]`. */
  toSubmissionEntry(): SecretEnvSubmissionEntry {
    if (this.kind === "ref") {
      return { ref: this.handle! };
    }
    if (this.#consumed) {
      throw new Error("Secret: this ephemeral secret was consumed by upload(); reference it via the returned Secret.ref instead");
    }
    return { ephemeral: true };
  }

  /** Raw value for the vaulted `secrets.envSecrets[<envName>]` (ephemeral only); `undefined` for refs. */
  toSecretValue(): string | undefined {
    if (this.kind === "ref") {
      return undefined;
    }
    if (this.#consumed) {
      throw new Error("Secret: this ephemeral secret was consumed by upload(); reference it via the returned Secret.ref instead");
    }
    return this.#value?.unwrap();
  }

  /** Redacted — never prints the value. */
  toString(): string {
    return this.kind === "ref" ? `Secret.ref(${this.handle})` : "Secret.value([REDACTED])";
  }

  /** Redacted — never serialises the value. */
  toJSON(): unknown {
    return this.kind === "ref" ? { kind: "ref", handle: this.handle } : { kind: "value", value: "[REDACTED]" };
  }
}

/**
 * Split `secretEnv: Record<envName, Secret>` into the value-free declarations
 * (`submission.secretEnv`) and the per-run vaulted values
 * (`secrets.envSecrets`). Mirrors {@link splitProxyEndpoints}: declarations ride
 * the hashed submission, ephemeral values ride the secrets channel
 * (hash-excluded). Refs contribute only a declaration.
 *
 * Throws on a non-`Secret` value or an env var name that won't match the
 * platform's `SECRET_ENV_NAME_PATTERN`, so the caller gets a precise error at
 * the call site instead of an HTTP 400 a round-trip later.
 */
export function splitSecretEnv(secretEnv: Readonly<Record<string, Secret>> | undefined): {
  declarations: Record<string, SecretEnvSubmissionEntry>;
  values: Record<string, string>;
} {
  const declarations: Record<string, SecretEnvSubmissionEntry> = {};
  const values: Record<string, string> = {};
  if (!secretEnv) {
    return { declarations, values };
  }
  for (const [envName, secret] of Object.entries(secretEnv)) {
    if (!(secret instanceof Secret)) {
      throw new TypeError(
        `secretEnv[${envName}] must be a Secret built via Secret.value(...) or Secret.ref(...)`
      );
    }
    if (!SECRET_ENV_NAME_PATTERN.test(envName)) {
      throw new Error(
        `secretEnv key ${JSON.stringify(envName)} must be a valid env var name matching ${SECRET_ENV_NAME_PATTERN.source}`
      );
    }
    declarations[envName] = secret.toSubmissionEntry();
    const value = secret.toSecretValue();
    if (value !== undefined) {
      values[envName] = value;
    }
  }
  return { declarations, values };
}
