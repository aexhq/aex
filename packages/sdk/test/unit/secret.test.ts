/**
 * SDK shape tests for Secret — the per-session/workspace secret reference builder.
 *
 * Secrets share the lifecycle SEMANTIC of Skill / File / Instructions: per-session by
 * default (vaulted at submit, gone when the session finishes), or an explicit
 * reference to a secret persisted through `aex.workspace.secrets`.
 *
 *   - Secret.value("sk-...")  = EPHEMERAL per-session value. Wire { ephemeral: true }
 *                               (value-free placeholder) + the value split into
 *                               the vaulted secrets channel, excluded from the
 *                               idempotency hash — exactly how McpServer splits
 *                               headers into secrets.mcpServers. Deleted at the
 *                               session's terminal (no workspace dependency).
 *   - Secret.ref("serper")    = WORKSPACE handle ref; wire { ref: "serper" }.
 *                               Value resolved server-side; NO value travels.
 *
 * Used as the values of `secretEnv: Record<envName, Secret>`; the client keys
 * the split by env-var name at submit time.
 */
import { describe, expect, it } from "bun:test";
import { SecretString } from "@aexhq/contracts";

import { Secret } from "../../src/secret.js";

describe("Secret.value (ephemeral, per-session — the default)", () => {
  it("toSubmissionEntry is a value-free placeholder; the value only via toSecretValue", () => {
    const s = Secret.value("sk-secret-123");
    expect(s.kind).toBe("value");
    expect(s.toSubmissionEntry()).toEqual({ ephemeral: true });
    expect(s.toSecretValue()).toBe("sk-secret-123");
  });

  it("accepts a SecretString value", () => {
    const s = Secret.value(new SecretString("sk-secret-123"));
    expect(s.toSecretValue()).toBe("sk-secret-123");
  });

  it("never leaks the value via toString / JSON (no accidental log leak)", () => {
    const s = Secret.value("sk-secret-123");
    expect(String(s)).not.toContain("sk-secret-123");
    expect(JSON.stringify({ s })).not.toContain("sk-secret-123");
  });

  it("rejects an empty value", () => {
    expect(() => Secret.value("")).toThrow();
  });
});

describe("Secret.ref (workspace handle — persisted, searchable by name)", () => {
  it("toSubmissionEntry returns the handle ref; no value travels", () => {
    const s = Secret.ref("serper");
    expect(s.kind).toBe("ref");
    expect(s.handle).toBe("serper");
    expect(s.toSubmissionEntry()).toEqual({ ref: "serper" });
    expect(s.toSecretValue()).toBeUndefined();
  });

  it("rejects an invalid or empty handle", () => {
    expect(() => Secret.ref("bad handle!")).toThrow();
    expect(() => Secret.ref("")).toThrow();
  });
});

describe("Secret persistence boundary", () => {
  it("does not make a submission value responsible for workspace writes", () => {
    const secret = Secret.value("sk-live-XYZ");
    expect("upload" in secret).toBe(false);
    expect("isConsumed" in secret).toBe(false);
  });
});

function compileTimeSecretHasNoUpload(secret: Secret): void {
  // @ts-expect-error Workspace persistence belongs to aex.workspace.secrets.set.
  void secret.upload;
}
void compileTimeSecretHasNoUpload;

function compileTimeSecretUsesNamedBuilders(): void {
  // @ts-expect-error Callers choose Secret.value(...) or Secret.ref(...).
  new Secret({ kind: "ref", handle: "serper" });
}
void compileTimeSecretUsesNamedBuilders;

describe("Secret split invariant (the leak-safety property)", () => {
  it("ref: the handle is hashable, the value is absent (resolved server-side)", () => {
    const s = Secret.ref("doubao");
    const publicWire = JSON.stringify(s.toSubmissionEntry());
    expect(publicWire).toContain("doubao"); // handle rides the (hashed) submission
    expect(s.toSecretValue()).toBeUndefined();
  });

  it("ephemeral: the value is NEVER in the submission entry (hash-excluded), only in the secret value", () => {
    const s = Secret.value("sk-live-XYZ");
    const publicWire = JSON.stringify(s.toSubmissionEntry());
    expect(publicWire).not.toContain("sk-live-XYZ"); // value never in the hashed submission
    expect(s.toSecretValue()).toBe("sk-live-XYZ"); // value only on the vaulted side
  });
});
