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
    ["postgres connection string", "DB=postgresql://postgres:s3cr3tPassw0rd@db.example.test:5432/postgres now", "s3cr3tPassw0rd"],
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

  it("redacts an aex self-describing workspace key as one label (whole 6-part shape)", () => {
    const key = "aex_dev_euw1_abc123def456_SeCr3tValue123_z9";
    const out = redactString(`AEX_API_KEY=${key} then run`);
    expect(out).not.toContain(key);
    expect(out).toContain(REDACTED);
    expect(containsSecretLikeValue(key)).toBe(true);
    // The canonical dashboard URL must survive (no `aex_<plane>_` shape).
    expect(redactString("https://api.aex.dev/api/sessions")).toBe("https://api.aex.dev/api/sessions");
  });

  it("redacts an aex account PAT by its aexu_ prefix", () => {
    const pat = "aexu_abcDEF123456ghiJKL789mnoPQR";
    const out = redactString(`token ${pat} done`);
    expect(out).not.toContain(pat);
    expect(out).toContain(REDACTED);
    expect(containsSecretLikeValue(pat)).toBe(true);
  });

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
    const dbUrl = "postgresql://postgres.example_project:LongPasswordValue9999@db-pool.example.test:5432/postgres";
    const out = redactString(dbUrl);
    expect(out).not.toContain("LongPasswordValue9999");
  });

  describe("benign text is preserved", () => {
    const benign = [
      "the quick brown fox jumps over the lazy dog",
      "rejects dotted event-name segments",
      "compatibility_date = 2026-05-20",
      "https://api.aex.dev/api/sessions",
      "session-1234-terminal",
      "local database status shows API URL http://127.0.0.1:56321",
      // Long digit-free mixed-case identifiers (stack-trace frames / API symbol
      // names the diagnostic bundle captures) must survive — eating them guts
      // debuggability. These are 24-39 chars, mixed-case, no digit.
      "at async asyncEntryPointWithESMLoader (node:internal/main)",
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

describe("value-shape precision: low-entropy NAMES survive, opaque secrets still mask (WS7 #5)", () => {
  // A customer secret NAME / timestamped slug / dashed identifier is NOT a
  // credential — masking it makes `secret_not_found` unactionable. These must
  // survive the entropy gate verbatim, INCLUDING a real `Date.now()` timestamp
  // whose whole-run entropy (~4.1) would trip a naive floor — the '-'-aware gate
  // judges each segment (word / decimal number), none of which is opaque.
  const names = [
    "spike17-race-1720000000000", // the exact repro (round timestamp, whole-run entropy ~3.1)
    "spike17-race-1720394857263", // real Date.now() variant (whole-run entropy ~4.1)
    "prod-db-primary-replica-secret", // dictionary kebab ending in the word "secret"
    "worker-config-2026-07-05", // words + a dashed date
    "my-super-long-descriptive-name" // digit-free kebab, 30 chars
  ];
  for (const name of names) {
    it(`preserves the low-entropy identifier "${name}"`, () => {
      expect(redactString(name)).toBe(name);
      expect(containsSecretLikeValue(name)).toBe(false);
    });
    it(`preserves "${name}" inside a secret_not_found error message`, () => {
      const msg = `referenced workspace secret ${JSON.stringify(name)} does not exist; create it via POST /api/secrets`;
      expect(redactString(msg)).toBe(msg);
    });
  }

  // Opaque secrets STILL mask — including ones that merely CONTAIN a '-' (the
  // base64url anti-regression: '-'-splitting must not open a leak). A neutral,
  // keyword-free, space-bounded prefix isolates the entropy gate (no SECRET_PATTERN
  // shape/keyword match), so these assert the generic high-entropy path itself.
  const opaque: ReadonlyArray<readonly [name: string, secret: string]> = [
    ["32-char base64 key (no dash) — entropy path", "Zx9Kq2Lp7Vn4Rt6Wy8Ub3Mc5Ad1Ef0Gh"],
    ["base64url secret CONTAINING a dash — entropy path", "Zx9Kq2Lp7Vn4Rt6-Wy8Ub3Mc5Ad1Ef0Gh"],
    ["32-char hex key (bare, not ses_ prefixed) — entropy path", "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6"],
    ["sk- sentinel with dashes — shape path", "sk-SENTINEL-9f3a1b2c3d4e5f6a7b8c9d0e1f2"]
  ];
  for (const [name, secret] of opaque) {
    it(`still masks ${name}`, () => {
      const out = redactString(`blob ${secret} end`);
      expect(out).not.toContain(secret);
      expect(out).toContain(REDACTED);
      expect(containsSecretLikeValue(secret)).toBe(true);
    });
  }
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
    expect(containsSecretLikeValue("session-1234-terminal")).toBe(false);
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
    const text = "line one\nline two\nsession-1234-terminal\n";
    expect(await runThrough([text])).toBe(text);
  });
});

describe("property: generated high-entropy tokens always redact", { timeout: 0 }, () => {
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
