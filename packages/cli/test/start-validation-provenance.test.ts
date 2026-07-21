import { resolve as resolvePath } from "node:path";
import { describe, expect, it } from "vitest";
import { PROVIDERS } from "@aexhq/contracts";
import { executeCli } from "../src/main.js";
import { makeIo } from "./support.js";

const COMMON = ["--api-key", "token", "--aex-url", "https://api.example.test"];
const IDEMPOTENCY_KEY_MAX_LENGTH = 255;
const VALID_START = [
  "--model", "claude-haiku-4-5",
  "--prompt", "hello",
  "--anthropic-api-key", "provider-key"
] as const;

interface ValidationCase {
  readonly label: string;
  readonly argv: readonly string[];
  readonly files?: Readonly<Record<string, string>>;
  readonly appendCommon?: boolean;
  readonly exitCode: 1 | 2;
  readonly code: "session_failed" | null;
  readonly flag: string;
  readonly message: string;
}

const cases: readonly ValidationCase[] = [
  {
    label: "common host",
    argv: [...VALID_START],
    appendCommon: false,
    exitCode: 2,
    code: null,
    flag: "--api-key",
    message: "aex start --api-key: no API key — pass --api-key or run `aex login`"
  },
  {
    label: "parse",
    argv: ["--provider", "anthropicc", ...VALID_START],
    exitCode: 2,
    code: null,
    flag: "--provider",
    message:
      `aex start --provider: must be one of: ${PROVIDERS.join(", ")} (got: anthropicc); ` +
      'did you mean "anthropic"?'
  },
  {
    label: "config",
    argv: ["--prompt", "hello", "--anthropic-api-key", "provider-key"],
    exitCode: 2,
    code: null,
    flag: "--model",
    message: "aex start --model: is required when --config is not provided"
  },
  {
    label: "attachment",
    argv: [...VALID_START, "--skill", "@bad-skill.md"],
    files: { [resolvePath("/tmp/cli-test", "bad-skill.md")]: "no frontmatter" },
    exitCode: 2,
    code: null,
    flag: "--skill",
    message: "aex start --skill: a skill name is required"
  },
  {
    label: "submission source-aware validator",
    argv: [...VALID_START, "--webhook", "http://hooks.example.test/aex"],
    exitCode: 1,
    code: "session_failed",
    flag: "--webhook",
    message: "aex start --webhook: webhook.url must use https (got http)"
  },
  {
    label: "submission structured field",
    argv: [...VALID_START, "--idempotency-key", "x".repeat(IDEMPOTENCY_KEY_MAX_LENGTH + 1)],
    exitCode: 1,
    code: "session_failed",
    flag: "--idempotency-key",
    message: `aex start --idempotency-key: idempotencyKey must be at most ${IDEMPOTENCY_KEY_MAX_LENGTH} characters`
  }
];

describe("aex start validation provenance", () => {
  it.each(cases)("maps $label failures to the exact command flag, envelope, and exit", async (row) => {
    const cap = makeIo({
      argv: ["start", ...row.argv, ...(row.appendCommon === false ? [] : COMMON)],
      ...(row.files ? { files: { ...row.files } } : {})
    });

    await executeCli(cap.io);

    expect(cap.exitCode).toBe(row.exitCode);
    expect(cap.stdout).toBe("");
    expect(cap.calls).toHaveLength(0);
    if (row.code === null) {
      expect(cap.stderr).toBe(`${row.message}\n`);
    } else {
      expect(JSON.parse(cap.stderr)).toEqual({ error: row.code, message: row.message });
    }
    expect(cap.stderr).toContain("aex start");
    expect(cap.stderr).toContain(row.flag);
    expect(cap.stderr).not.toMatch(/(?:Aex\.start|Skill\.fromContent|Tool\.fromFiles|Instructions\.fromContent|File\.fromBytes)/);
  });

  it("preserves first-error precedence when several user flags are invalid", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--provider", "anthropicc",
        "--runtime", "fargate",
        "--session-timeout", "1s",
        "--skill", "@missing.md",
        ...COMMON
      ]
    });

    await executeCli(cap.io);

    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toBe(
      `aex start --provider: must be one of: ${PROVIDERS.join(", ")} (got: anthropicc); did you mean "anthropic"?\n`
    );
    expect(cap.calls).toHaveLength(0);
  });
});
