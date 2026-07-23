import { describe, expect, it } from "bun:test";
import { redactSecrets, SecretString } from "../../src/index.js";

describe("secret redaction", () => {
  it("redacts known key patterns", () => {
    const redacted = redactSecrets({
      apiKey: "sk-ant-test-secretsecretsecretsecret",
      message: "Authorization header sk-ant-test-secretsecretsecretsecret"
    });

    expect(redacted).toEqual({ apiKey: "[REDACTED]", message: "Authorization header [REDACTED]" });
  });

  it("does not stringify SecretString values", () => {
    const secret = new SecretString("sk-ant-test-secretsecretsecretsecret");
    expect(`${secret}`).toBe("[REDACTED]");
    expect(JSON.stringify({ secret })).toContain("[REDACTED]");
  });
});
