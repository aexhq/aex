import { SECRET_ENV_NAME_PATTERN, SECRET_HANDLE_PATTERN, SecretString } from "@aexhq/contracts";

/**
 * Non-secret wire shape for one `secretEnv` entry. A workspace ref carries the
 * handle (hashable, resolved server-side); an ephemeral secret carries only a
 * value-free placeholder — the value is split into the vaulted secrets channel
 * and excluded from the idempotency hash.
 */
export type SecretEnvSubmissionEntry = { readonly ref: string } | { readonly ephemeral: true };

/**
 * A secret with the SAME lifecycle semantic as `File` / `Instructions`:
 * EPHEMERAL per-session by default, or a reference to a persisted workspace
 * secret created explicitly through `aex.workspace.secrets.set(...)`.
 *
 *   - `Secret.value(v)` — EPHEMERAL per-session: the value is vaulted when the session is created and
 *     excluded from the idempotency hash; only a `{ ephemeral: true }`
 *     placeholder rides the (hashed) submission. Deleted when the session finishes.
 *     Clean, no workspace dependency. ≙ `File.fromBytes(...)` (a draft).
 *   - `Secret.ref(handle)` — WORKSPACE: only the handle rides the submission;
 *     the value is resolved server-side from the workspace secret store. No
 *     value ever travels.
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

  /** Internal constructor. Use `Secret.value(...)` or `Secret.ref(...)`. */
  private constructor(args: { readonly kind: "value"; readonly value: SecretString } | { readonly kind: "ref"; readonly handle: string }) {
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

  /** Ephemeral per-session value. Vaulted when the session is created; never in the spec/hash; gone at terminal. */
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

  /** Non-secret wire entry for `submission.secretEnv[<envName>]`. */
  toSubmissionEntry(): SecretEnvSubmissionEntry {
    if (this.kind === "ref") {
      return { ref: this.handle! };
    }
    return { ephemeral: true };
  }

  /** Raw value for the vaulted `secrets.envSecrets[<envName>]` (ephemeral only); `undefined` for refs. */
  toSecretValue(): string | undefined {
    if (this.kind === "ref") {
      return undefined;
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
 * (`submission.secretEnv`) and the per-session vaulted values
 * (`secrets.envSecrets`). Secret declarations ride
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
