/**
 * Unit coverage for the `expectStructuredError` matcher that the live
 * failure-case tests assert with. The matcher itself lives beside the other
 * harness helpers in `test/_fixtures/`; its test lives here because
 * `test/offline/` is the lane CI actually collects (`test:user:offline`),
 * while `test/_fixtures/` is only swept locally.
 */
import { describe, expect, it } from "bun:test";
import { expectStructuredError } from "../_fixtures/structured-error.js";

describe("expectStructuredError", () => {
  it("accepts a named error class in the default allow-list", () => {
    expect(() =>
      expectStructuredError({ errorClass: "AexError", errorMessage: "boom" })
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
        { classes: ["AexError", "ValidationError"] }
      )
    ).toThrow(/not in allow-list/);
  });

  it("requires messageIncludes substring (case-insensitive)", () => {
    expect(() =>
      expectStructuredError(
        { errorClass: "AexError", errorMessage: "Runtime native_unsupported" },
        { messageIncludes: "native" }
      )
    ).not.toThrow();
    expect(() =>
      expectStructuredError(
        { errorClass: "AexError", errorMessage: "something else" },
        { messageIncludes: "native" }
      )
    ).toThrow(/does not include "native"/);
  });
});
