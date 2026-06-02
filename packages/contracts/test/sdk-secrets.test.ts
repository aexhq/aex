import { Buffer } from "node:buffer";
import { PassThrough } from "node:stream";
import { pipeline } from "node:stream/promises";
import fc from "fast-check";
import { describe, expect, it } from "vitest";
import {
  containsSecretLikeValue,
  createRedactingStream,
  redactSecrets,
  redactString,
  SecretString
} from "../src/index.js";

const REDACTED = "[REDACTED]";

describe("redactString — value-agnostic shapes", () => {
  // Every case asserts the literal secret is GONE, not merely that some
  // [REDACTED] appears — a partial mask that leaves a substring is the
  // exact failure this redactor exists to prevent.
  const cases: ReadonlyArray<readonly [name: string, input: string, leak: string]> = [
    ["sk-ant key (tool-emitted, never seeded)", "key=sk-ant-api03-aBcD1234efGh5678ijKlmnop end", "sk-ant-api03-aBcD1234efGh5678ijKlmnop"],
    ["openai sk- key", "OPENAI=sk-proj-abcdefghijklmnop1234567890ABCD", "sk-proj-abcdefghijklmnop1234567890ABCD"],
    ["apt_ workspace token", "token apt_abcdEFGH1234ijklMNOP5678 trailing", "apt_abcdEFGH1234ijklMNOP5678"],
    ["JWT", "Cookie: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dozjgNryP4J3jVmNHl0w5N", "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dozjgNryP4J3jVmNHl0w5N"],
    ["postgres connection string", "DB=postgresql://postgres:s3cr3tPassw0rd@db.poozsebwtfvdchhtixuq.supabase.co:5432/postgres now", "s3cr3tPassw0rd"],
    ["AWS access key id", "id AKIAIOSFODNN7EXAMPLE done", "AKIAIOSFODNN7EXAMPLE"],
    ["slack token", "xoxb-1234567890-abcdefghijkl rest", "xoxb-1234567890-abcdefghijkl"]
  ];

  for (const [name, input, leak] of cases) {
    it(`redacts ${name}`, () => {
      const out = redactString(input);
      expect(out).not.toContain(leak);
      expect(out).toContain(REDACTED);
    });
  }

  it("redacts an Authorization: Bearer header value but keeps the header name", () => {
    const out = redactString("Authorization: Bearer abcDEF123456ghiJKL789mnoPQR");
    expect(out).not.toContain("abcDEF123456ghiJKL789mnoPQR");
    expect(out).toContain("Authorization: Bearer");
    expect(out).toContain(REDACTED);
  });

  it("redacts a high-entropy token nobody named a pattern for", () => {
    // Base64url-ish 40-char run; no recognised prefix.
    const blob = "Zx9Kq2Lp7Vn4Rt6Wy8Ub3Mc5Ad1Ef0Gh2Ij4Kl6";
    const out = redactString(`leaked=${blob} ok`);
    expect(out).not.toContain(blob);
    expect(out).toContain(REDACTED);
  });

  it("redacts the substring-of-a-known-secret case (the sed-mask leak)", () => {
    // A naive mask might blank only the prefix and leave the password tail as a
    // substring. The pg-URI shape redacts the whole URI, killing the tail.
    const dbUrl = "postgresql://postgres.poozsebwtfvdchhtixuq:LongPasswordValue9999@aws-1.pooler.supabase.com:5432/postgres";
    const out = redactString(dbUrl);
    expect(out).not.toContain("LongPasswordValue9999");
  });

  describe("benign text is preserved", () => {
    const benign = [
      "the quick brown fox jumps over the lazy dog",
      "rejects dotted event-name segments",
      "compatibility_date = 2026-05-20",
      "https://api.antpath.ai/v1/runs",
      "run-1234-terminal",
      "supabase status shows API URL http://127.0.0.1:56321",
      // Long digit-free mixed-case identifiers (stack-trace frames / API symbol
      // names the diagnostic bundle captures) must survive — eating them guts
      // debuggability. These are 24-39 chars, mixed-case, no digit.
      "at async asyncRunEntryPointWithESMLoader (node:internal/main)",
      "createDurableObjectNamespace and getReadableStreamController"
    ];
    for (const text of benign) {
      it(`keeps "${text.slice(0, 32)}…"`, () => {
        expect(redactString(text)).toBe(text);
      });
    }
  });

  it("masks known values supplied for belt-and-suspenders", () => {
    const out = redactString("the value is hunter2hunter2 here", ["hunter2hunter2"]);
    expect(out).not.toContain("hunter2hunter2");
    expect(out).toContain(REDACTED);
  });

  it("ignores trivially short known values", () => {
    // A 3-char "known" value must not blank ordinary text.
    expect(redactString("the cat sat", ["cat"])).toBe("the cat sat");
  });
});

describe("redactSecrets — structured values", () => {
  it("redacts known key patterns inside objects", () => {
    const redacted = redactSecrets({
      apiKey: "sk-ant-test-secretsecretsecretsecret",
      message: "Authorization header sk-ant-test-secretsecretsecretsecret"
    });
    expect(JSON.stringify(redacted)).not.toContain("sk-ant-test");
    expect(JSON.stringify(redacted)).toContain(REDACTED);
  });

  it("redacts by key name even when the value is not secret-shaped", () => {
    const redacted = redactSecrets({ password: "shortpw" });
    expect(redacted.password).toBe(REDACTED);
  });
});

describe("SecretString", () => {
  it("never stringifies its value", () => {
    const secret = new SecretString("sk-ant-test-secretsecretsecretsecret");
    expect(`${secret}`).toBe(REDACTED);
    expect(JSON.stringify({ secret })).toContain(REDACTED);
    expect(secret.unwrap()).toBe("sk-ant-test-secretsecretsecretsecret");
  });
});

describe("containsSecretLikeValue", () => {
  it("flags secret shapes and high-entropy runs", () => {
    expect(containsSecretLikeValue("sk-ant-api03-aBcD1234efGh5678ijKlmnop")).toBe(true);
    expect(containsSecretLikeValue("Zx9Kq2Lp7Vn4Rt6Wy8Ub3Mc5Ad1Ef0Gh2Ij4Kl6")).toBe(true);
  });
  it("does not flag benign text", () => {
    expect(containsSecretLikeValue("the quick brown fox")).toBe(false);
    expect(containsSecretLikeValue("run-1234-terminal")).toBe(false);
  });
});

describe("createRedactingStream — stream-before-disk", () => {
  async function runThrough(chunks: readonly string[], known: string[] = []): Promise<string> {
    const source = new PassThrough();
    const sink = new PassThrough();
    const collected: Buffer[] = [];
    sink.on("data", (c: Buffer) => collected.push(c));
    const done = pipeline(source, createRedactingStream(known), sink);
    for (const chunk of chunks) {
      source.write(chunk);
    }
    source.end();
    await done;
    return Buffer.concat(collected).toString();
  }

  it("redacts a secret that arrives whole", async () => {
    const out = await runThrough(["key=sk-ant-api03-aBcD1234efGh5678ijKlmnop\n"]);
    expect(out).not.toContain("sk-ant-api03-aBcD1234efGh5678ijKlmnop");
    expect(out).toContain(REDACTED);
  });

  it("redacts a secret split across a chunk boundary (line buffering)", async () => {
    // The secret is split mid-token across two writes with no newline between
    // them; line buffering must hold the partial line until it completes.
    const out = await runThrough(["key=sk-ant-api03-aBcD123", "4efGh5678ijKlmnop\n"]);
    expect(out).not.toContain("sk-ant-api03-aBcD1234efGh5678ijKlmnop");
    expect(out).toContain(REDACTED);
  });

  it("flushes a trailing line with no final newline", async () => {
    const out = await runThrough(["no newline key=sk-ant-api03-aBcD1234efGh5678ijKlmnop"]);
    expect(out).not.toContain("sk-ant-api03-aBcD1234efGh5678ijKlmnop");
    expect(out).toContain(REDACTED);
  });

  it("preserves benign multi-line output unchanged", async () => {
    const text = "line one\nline two\nrun-1234-terminal\n";
    expect(await runThrough([text])).toBe(text);
  });
});

describe("property: generated high-entropy tokens always redact", () => {
  it("never leaks a >=24-char high-entropy base64url token", () => {
    fc.assert(
      fc.property(
        fc.string({ unit: fc.constantFrom(..."ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_".split("")), minLength: 28, maxLength: 64 }),
        (token) => {
          // Only assert on tokens that are genuinely high-entropy; the gate is
          // deliberate (we do not want to redact "aaaaaaaa…"). Filter the
          // degenerate low-entropy strings fast-check may generate.
          fc.pre(containsSecretLikeValue(token));
          const out = redactString(`prefix ${token} suffix`);
          return !out.includes(token) && out.includes(REDACTED);
        }
      ),
      { numRuns: 500 }
    );
  });
});
