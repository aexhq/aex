import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { UnknownFieldError, parseInlineSecrets } from "../src/internal.js";
import * as customerContracts from "../src/index.js";
// @ts-expect-error Structured parser diagnostics are workspace-internal metadata.
import { UnknownFieldError as PublicUnknownFieldError } from "../src/index.js";
void PublicUnknownFieldError;

function thrownBy(invoke: () => unknown): unknown {
  try {
    invoke();
  } catch (error) {
    return error;
  }
  throw new Error("expected invocation to throw");
}

describe("structured unknown-field diagnostics", () => {
  it("keeps the dependency-free owner on the workspace-internal entrypoint only", () => {
    const owner = readFileSync(new URL("../src/unknown-field-error.ts", import.meta.url), "utf8");
    const internal = readFileSync(new URL("../src/internal.ts", import.meta.url), "utf8");
    const customer = readFileSync(new URL("../src/index.ts", import.meta.url), "utf8");

    expect(owner).not.toMatch(/^import /m);
    expect(internal).toContain('export * from "./unknown-field-error.js";');
    expect(customer).not.toContain("unknown-field-error");
    expect(customerContracts).not.toHaveProperty("UnknownFieldError");
  });

  it("preserves Error compatibility and immutable ordered field data", () => {
    const callerKeys = ["alpha", "beta"];
    const error = new UnknownFieldError("object", "unknown", callerKeys);

    expect(error).toBeInstanceOf(Error);
    expect(error.name).toBe("Error");
    expect(error.message).toBe("object.unknown is not an allowed field; permitted: alpha, beta");
    expect(error.objectPath).toBe("object");
    expect(error.unknownKey).toBe("unknown");
    expect(error.permittedKeys).toEqual(["alpha", "beta"]);
    expect(error.permittedKeys).not.toBe(callerKeys);
    expect(Object.isFrozen(error.permittedKeys)).toBe(true);

    callerKeys.push("later");
    expect(error.permittedKeys).toEqual(["alpha", "beta"]);
  });

  it("rebuilds the same diagnostic from a caller-owned complete key order", () => {
    const original = new UnknownFieldError("secrets", "unknown", ["apiKeys", "envSecrets"]);
    const expanded = original.withPermittedKeys(["apiKeys", "proxyEndpointAuth", "envSecrets"]);

    expect(expanded).toBeInstanceOf(UnknownFieldError);
    expect(expanded).not.toBe(original);
    expect(expanded.objectPath).toBe("secrets");
    expect(expanded.unknownKey).toBe("unknown");
    expect(expanded.permittedKeys).toEqual(["apiKeys", "proxyEndpointAuth", "envSecrets"]);
    expect(expanded.message).toBe(
      "secrets.unknown is not an allowed field; permitted: apiKeys, proxyEndpointAuth, envSecrets"
    );
  });

  it("gives the public inline-secrets parser structured data with exact legacy prose", () => {
    const error = thrownBy(() => parseInlineSecrets({ unknown: true }));

    expect(error).toBeInstanceOf(UnknownFieldError);
    expect(error).toMatchObject({
      name: "Error",
      message: "secrets.unknown is not an allowed field; permitted: mcpServers, envSecrets",
      objectPath: "secrets",
      unknownKey: "unknown",
      permittedKeys: ["mcpServers", "envSecrets"]
    });
  });

  it("preserves first-key order, reserved-namespace precedence, and accepted fields", () => {
    const first = thrownBy(() => parseInlineSecrets({ later: true, earlier: true }));
    expect(first).toMatchObject({ unknownKey: "later" });

    const reserved = thrownBy(() => parseInlineSecrets({ __aex_internal: true, unknown: true }));
    expect(reserved).toBeInstanceOf(Error);
    expect(reserved).not.toBeInstanceOf(UnknownFieldError);
    expect((reserved as Error).message).toBe(
      "secrets.__aex_internal uses the platform-internal __aex_ namespace and may not be set by callers"
    );

    expect(parseInlineSecrets({
      mcpServers: [{
        name: "remote",
        url: "https://mcp.example.test",
        headers: { Authorization: "safe-placeholder" }
      }],
      envSecrets: { SERVICE_TOKEN: "safe-placeholder" }
    })).toEqual({
      mcpServers: [{
        name: "remote",
        url: "https://mcp.example.test",
        headers: { Authorization: "safe-placeholder" }
      }],
      envSecrets: { SERVICE_TOKEN: "safe-placeholder" }
    });
  });
});
