import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";
import {
  formatChildFailure,
  LIVE_REQUEST_TRACE_SOURCE,
  REDACTED,
  redactKnownValues
} from "../_fixtures/live-diagnostics.js";

describe("live child-runner diagnostics", () => {
  it("formats request traces with public-safe method/path/status/duration only", () => {
    const writes: string[] = [];
    const context = {
      Date,
      URL,
      process: {
        env: {
          AEX_API_KEY: "aex-live-token-secret",
          DEEPSEEK_KEY: "deepseek-live-key-secret"
        },
        stderr: {
          write(line: string): void {
            writes.push(line);
          }
        }
      }
    };

    runInNewContext(
      `${LIVE_REQUEST_TRACE_SOURCE}
      writeRequestTrace(
        "post",
        "https://api.example.test/api/sessions/session-1/messages?ticket=signed-url-secret",
        404,
        Date.now() - 12
      );
      aexDebug(
        "[aex] POST /api/sessions -> 404 9ms " +
          process.env.AEX_API_KEY +
          " " +
          process.env.DEEPSEEK_KEY
      );
      `,
      context
    );

    const output = writes.join("");
    expect(output).toMatch(/\[aex-user-test\] POST \/api\/sessions\/session-1\/messages -> 404 \d+ms/);
    expect(output).toContain("[aex] POST /api/sessions -> 404 9ms");
    expect(output).not.toContain("ticket=");
    expect(output).not.toContain("signed-url-secret");
    expect(output).not.toContain("aex-live-token-secret");
    expect(output).not.toContain("deepseek-live-key-secret");
    expect(output).toContain(REDACTED);
  });

  it("redacts known live credentials before parent failures include child output", () => {
    const formatted = formatChildFailure(
      "chat-session raw runner",
      {
        exitCode: 1,
        stdout: "body contained deepseek-live-key-secret",
        stderr: "[aex] POST /api/sessions -> 404 10ms Authorization: Bearer aex-live-token-secret"
      },
      ["aex-live-token-secret", "deepseek-live-key-secret"]
    );

    expect(formatted).toContain("chat-session raw runner exited 1");
    expect(formatted).toContain("POST /api/sessions -> 404");
    expect(formatted).not.toContain("aex-live-token-secret");
    expect(formatted).not.toContain("deepseek-live-key-secret");
    expect(redactKnownValues("safe", ["cat"])).toBe("safe");
  });
});
