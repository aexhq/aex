import { describe, expect, it } from "vitest";
import { redactSecrets, SecretString } from "../../src/index.js";

describe("secret redaction", () => {
  it("redacts known key patterns", () => {
    const redacted = redactSecrets({
      apiKey: "sk-ant-test-secretsecretsecretsecret",
      message: "Authorization header sk-ant-test-secretsecretsecretsecret"
    });

    expect(JSON.stringify(redacted)).not.toContain("sk-ant-test");
    expect(JSON.stringify(redacted)).toContain("[REDACTED]");
  });

  it("does not stringify SecretString values", () => {
    const secret = new SecretString("sk-ant-test-secretsecretsecretsecret");
    expect(`${secret}`).toBe("[REDACTED]");
    expect(JSON.stringify({ secret })).toContain("[REDACTED]");
  });
});
