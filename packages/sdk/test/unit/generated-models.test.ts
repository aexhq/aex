import { describe, expect, test } from "bun:test";

import {
  assertId,
  isId,
  newId,
  type Id,
  type SessionCreateRequest,
} from "../../src/index.js";

describe("generated public models", () => {
  test("exports authored request types and canonical UUIDv7 constructors", () => {
    const operationId: Id<"operation"> = newId("operation");
    const request: SessionCreateRequest = {
      provider: "openai",
      model: "gpt-5",
      providerCredentialId: newId("provider_credential"),
    };

    expect(isId("operation", operationId)).toBeTrue();
    expect(assertId("provider_credential", request.providerCredentialId))
      .toBe(request.providerCredentialId);
  });
});
