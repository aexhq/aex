import { inspect } from "node:util";
import { describe, expect, it } from "vitest";
import { SessionConfigValidationError } from "@aexhq/contracts";
import {
  validatedSessionConfig,
  type SessionConfigDiagnosticPolicy
} from "../../src/session-validate.js";

const scalar = (...rejectedValues: readonly unknown[]): SessionConfigDiagnosticPolicy => ({
  kind: "scalar",
  rejectedValues
});

function rejected(
  thrown: unknown,
  policy: SessionConfigDiagnosticPolicy = scalar("caller-secret-q7")
): SessionConfigValidationError {
  try {
    validatedSessionConfig(
      "aex.sessions.create",
      "runtime.size",
      "runtime.size must be a supported size preset",
      () => {
        throw thrown;
      },
      policy
    );
  } catch (error) {
    return error as SessionConfigValidationError;
  }
  throw new Error("expected validation rejection");
}

describe("typed session-config validation adapter", () => {
  it("returns the validator's exact result identity and inferred type", () => {
    const value = { kind: "parsed" as const, count: 1 as const };
    const parsed = validatedSessionConfig(
      "aex.sessions.create",
      "runtime.size",
      "runtime.size must be a supported size preset",
      () => value,
      scalar()
    );
    const kind: "parsed" = parsed.kind;
    const count: 1 = parsed.count;
    void [kind, count];
    expect(parsed).toBe(value);
  });

  it("keeps the top-level contract exact and installs a fresh bounded cause", () => {
    const raw = Object.assign(
      new Error('runtimeSize must be one of: 0.5cpu-1gb, 1cpu-2gb (got "caller-secret-q7")'),
      { details: { raw: "caller-secret-q7" }, extra: "caller-secret-q7" }
    );
    const error = rejected(raw);
    const cause = error.cause as Error;

    expect(error).toMatchObject({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      message: "aex.sessions.create: runtime.size must be a supported size preset",
      details: { field: "runtime.size" }
    });
    expect(cause).toBeInstanceOf(Error);
    expect(cause).not.toBe(raw);
    expect(cause.name).toBe("SessionConfigDiagnosticError");
    expect(cause.message).toContain("runtimeSize must be one of");
    expect(cause.message).not.toContain("caller-secret-q7");
    expect(cause.message.length).toBeLessThanOrEqual(512);
    expect(cause.cause).toBeUndefined();
    expect(Object.keys(cause)).toEqual([]);
    expect(cause).not.toHaveProperty("details");
    expect(cause).not.toHaveProperty("extra");
    expect(Object.keys(error)).toEqual(["code", "details", "name"]);
    expect(JSON.stringify(error)).not.toContain("caller-secret-q7");
    expect(inspect(error)).not.toContain("caller-secret-q7");
    expect(inspect(cause)).not.toContain("caller-secret-q7");
    const clonedEvidence = typeof structuredClone === "function"
      ? inspect(structuredClone(error))
      : "structuredClone unavailable";
    expect(clonedEvidence).not.toContain("caller-secret-q7");
  });

  it.each([
    ["one character", "§"],
    ["quoted and repeated", "quoted-q7"],
    ["unicode", "秘密-q7"],
    ["URL-looking", "https%3A%2F%2Fuser%3Apass%40example.test%2Fq7"],
    ["control-bearing", "line-q7\nnext\u0000part"],
    ["high entropy", "Q7w9Er2Ty5Ui8Op1As4Df7Gh0Jk3Lz6Xc9Vb2Nm5"]
  ])("exact-masks %s rejected tokens", (_label, token) => {
    const error = rejected(new Error(`validator rejected ${token}; allowed: alpha, beta`), scalar(token));
    const cause = error.cause as Error;
    expect(cause.message).not.toContain(token);
    expect(inspect(error)).not.toContain(token);
  });

  it("flattens controls and caps long diagnostics", () => {
    const controls = rejected(new Error("allowed:\nalpha\tbeta\u0000gamma"), scalar());
    expect((controls.cause as Error).message).not.toMatch(/[\u0000-\u001f\u007f-\u009f]/);

    const long = rejected(new Error(`allowed values: ${"safe-value ".repeat(100)}`), scalar());
    expect((long.cause as Error).message.length).toBeLessThanOrEqual(512);
  });

  it("uses field-only fallback for forced-redacted and unsafe diagnostics", () => {
    const fixed = "underlying validator rejected responseFormat; diagnostic redacted";
    const hidden = new Error("must never be read");
    Object.defineProperty(hidden, "message", {
      get: () => {
        throw new Error("message getter escaped");
      }
    });
    const forced = (() => {
      try {
        validatedSessionConfig(
          "aex.sessions.create",
          "responseFormat",
          "responseFormat is invalid",
          () => {
            throw hidden;
          },
          { kind: "redacted" }
        );
      } catch (error) {
        return error as SessionConfigValidationError;
      }
      throw new Error("expected rejection");
    })();
    expect((forced.cause as Error).message).toBe(fixed);

    for (const value of [null, { message: "object secret" }, "", hidden]) {
      const error = rejected(value, scalar());
      expect((error.cause as Error).message).toBe(
        "underlying validator rejected runtime.size; diagnostic redacted"
      );
    }

    const hostileValues = new Proxy([] as unknown[], {
      get() {
        throw new Error("token collection escaped");
      }
    });
    const hostile = rejected(new Error("safe diagnostic"), {
      kind: "scalar",
      rejectedValues: hostileValues
    });
    expect((hostile.cause as Error).message).toBe(
      "underlying validator rejected runtime.size; diagnostic redacted"
    );
  });

  it("accepts a thrown string but never retains nonstandard throwable objects", () => {
    const fromString = rejected("allowed presets: small, medium", scalar());
    expect((fromString.cause as Error).message).toContain("allowed presets");

    const raw = { toString: () => "caller-secret-q7" };
    const fromObject = rejected(raw);
    expect((fromObject.cause as Error).message).toBe(
      "underlying validator rejected runtime.size; diagnostic redacted"
    );
    expect(inspect(fromObject)).not.toContain("caller-secret-q7");
  });
});
