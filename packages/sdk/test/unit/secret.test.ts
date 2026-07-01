/**
 * SDK shape tests for Secret — the per-run/workspace secret reference builder.
 *
 * Secrets share the lifecycle SEMANTIC of Skill / File / AgentsMd: per-run by
 * default (vaulted at submit, gone when the run finishes), and PROMOTABLE to a
 * persisted, name-searchable workspace secret you can reference and reuse.
 *
 *   - Secret.value("sk-...")  = EPHEMERAL per-run value. Wire { ephemeral: true }
 *                               (value-free placeholder) + the value split into
 *                               the vaulted secrets channel, excluded from the
 *                               idempotency hash — exactly how McpServer splits
 *                               headers into secrets.mcpServers. Deleted at the
 *                               run's terminal (no workspace dependency).
 *   - secret.upload(client,…) = PROMOTE that value into the workspace secret
 *                               store under a name; returns a Secret.ref.
 *   - Secret.ref("serper")    = WORKSPACE handle ref; wire { ref: "serper" }.
 *                               Value resolved server-side; NO value travels.
 *
 * Used as the values of `secretEnv: Record<envName, Secret>`; the client keys
 * the split by env-var name at submit time.
 */
import { describe, expect, it } from "vitest";
import { SecretString } from "@aexhq/contracts";

import { Secret } from "../../src/secret.js";

describe("Secret.value (ephemeral, per-run — the default)", () => {
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

describe("secret.upload (promote ephemeral → persisted workspace ref)", () => {
  function makeUploader(): {
    uploader: { _createWorkspaceSecret(a: { name: string; value: string }): Promise<{ name: string }> };
    created: Array<{ name: string; value: string }>;
  } {
    const created: Array<{ name: string; value: string }> = [];
    const uploader = {
      async _createWorkspaceSecret(a: { name: string; value: string }) {
        created.push(a);
        return { name: a.name };
      }
    };
    return { uploader, created };
  }

  it("uploads the value under a name and returns a Secret.ref to reuse", async () => {
    const { uploader, created } = makeUploader();
    const ref = await Secret.value("sk-live-XYZ").upload(uploader, { name: "serper" });
    expect(created).toEqual([{ name: "serper", value: "sk-live-XYZ" }]);
    expect(ref.kind).toBe("ref");
    expect(ref.handle).toBe("serper");
    expect(ref.toSubmissionEntry()).toEqual({ ref: "serper" });
    // After promotion the value lives in the workspace store, not on the ref.
    expect(ref.toSecretValue()).toBeUndefined();
  });

  it("consumes the ephemeral secret so it can't also be submitted inline", async () => {
    const { uploader } = makeUploader();
    const s = Secret.value("sk-live-XYZ");
    await s.upload(uploader, { name: "serper" });
    expect(s.isConsumed).toBe(true);
    expect(() => s.toSecretValue()).toThrow();
    await expect(s.upload(uploader, { name: "serper" })).rejects.toThrow();
  });

  it("rejects uploading a workspace ref (only ephemeral secrets are uploadable)", async () => {
    const { uploader } = makeUploader();
    await expect(Secret.ref("serper").upload(uploader, { name: "x" })).rejects.toThrow();
  });

  it("validates the workspace name", async () => {
    const { uploader } = makeUploader();
    await expect(Secret.value("v").upload(uploader, { name: "bad name!" })).rejects.toThrow();
  });
});

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
