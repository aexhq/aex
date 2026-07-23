import { describe, expect, it } from "bun:test";
import {
  AexNetworkError,
  SessionConfigValidationError
} from "../src/sdk-errors.js";

describe("SessionConfigValidationError compatibility", () => {
  it("keeps legacy construction exact and causeless", () => {
    const error = new SessionConfigValidationError(
      "aex.sessions.create: runtime.size must be a supported size preset",
      { field: "runtime.size" }
    );

    expect(error).toMatchObject({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      message: "aex.sessions.create: runtime.size must be a supported size preset",
      details: { field: "runtime.size" }
    });
    expect(Object.isFrozen(error.details)).toBe(false);
    expect(Object.keys(error)).toEqual(["code", "details", "name"]);
    expect(JSON.parse(JSON.stringify(error))).toEqual({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      details: { field: "runtime.size" }
    });
    expect(error.cause).toBeUndefined();
  });

  it("adds native non-enumerable cause without widening details or JSON", () => {
    const diagnostic = new Error("bounded diagnostic");
    const error = new SessionConfigValidationError(
      "aex.sessions.create: runtime.size must be a supported size preset",
      { field: "runtime.size" },
      { cause: diagnostic }
    );

    expect(error.cause).toBe(diagnostic);
    expect(Object.getOwnPropertyDescriptor(error, "cause")).toMatchObject({ enumerable: false });
    expect(Object.keys(error)).toEqual(["code", "details", "name"]);
    expect(error.details).toEqual({ field: "runtime.size" });
    expect(Object.isFrozen(error.details)).toBe(false);
    expect(JSON.parse(JSON.stringify(error))).toEqual({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      details: { field: "runtime.size" }
    });
  });

  it("does not change raw transport-cause ownership", () => {
    const raw = Object.assign(new Error("connect failed"), { code: "ECONNREFUSED" });
    const network = new AexNetworkError({
      method: "POST",
      host: "example.test",
      path: "/api/sessions",
      cause: raw
    });

    expect(network.cause).toBe(raw);
  });
});
