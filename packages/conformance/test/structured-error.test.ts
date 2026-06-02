import { describe, expect, it } from "vitest";
import { expectStructuredError } from "../src/structured-error.js";

describe("expectStructuredError", () => {
  it("accepts a named error class in the default allow-list", () => {
    expect(() =>
      expectStructuredError({ errorClass: "AntpathError", errorMessage: "boom" })
    ).not.toThrow();
  });

  it("rejects a bare Error class (unstructured)", () => {
    expect(() => expectStructuredError({ errorClass: "Error", errorMessage: "boom" })).toThrow(
      /not structured/
    );
  });

  it("rejects null/undefined errorClass", () => {
    expect(() => expectStructuredError({ errorClass: null, errorMessage: "boom" })).toThrow(
      /not structured/
    );
    expect(() => expectStructuredError({ errorClass: undefined, errorMessage: "boom" })).toThrow(
      /not structured/
    );
  });

  it("rejects a class not in the allow-list when classes are provided", () => {
    expect(() =>
      expectStructuredError(
        { errorClass: "TypeError", errorMessage: "boom" },
        { classes: ["AntpathError", "ValidationError"] }
      )
    ).toThrow(/not in allow-list/);
  });

  it("requires messageIncludes substring (case-insensitive)", () => {
    expect(() =>
      expectStructuredError(
        { errorClass: "AntpathError", errorMessage: "Runtime native_unsupported" },
        { messageIncludes: "native" }
      )
    ).not.toThrow();
    expect(() =>
      expectStructuredError(
        { errorClass: "AntpathError", errorMessage: "something else" },
        { messageIncludes: "native" }
      )
    ).toThrow(/does not include "native"/);
  });
});
