import { describe, expect, it } from "bun:test";
import { executeCli } from "../src/main.js";
import { makeIo } from "./support.js";

const COMMON = ["--api-key", "test-token", "--aex-url", "https://api.example"];

describe("aex otel", () => {
  it("is discoverable with the canonical signal and JSON flags", async () => {
    const cap = makeIo({ argv: ["otel", "--help"] });
    await executeCli(cap.io);

    expect(cap.exitCode).toBe(0);
    expect(cap.stdout).toContain("aex otel <session-id>");
    expect(cap.stdout).toContain("--signal traces|logs");
    expect(cap.stdout).toContain("--json");
  });

  it("prints one standards-pure OTLP traces body and follows the cursor header", async () => {
    const cap = makeIo({
      argv: ["otel", "ses_cli", "--signal", "traces", "--json", ...COMMON],
      fetchHandler: ({ url }) => {
        const cursor = new URL(url).searchParams.get("cursor");
        return Response.json(
          { resourceSpans: [{ scopeSpans: [{ spans: [{ name: cursor === null ? "invoke_agent" : "execute_tool" }] }] }] },
          cursor === null ? { headers: { "x-aex-next-cursor": "page-2" } } : undefined
        );
      }
    });

    await executeCli(cap.io);

    expect(cap.exitCode).toBe(0);
    const output = JSON.parse(cap.stdout) as Record<string, unknown>;
    expect(Object.keys(output)).toEqual(["resourceSpans"]);
    expect(JSON.stringify(output)).toContain("invoke_agent");
    expect(JSON.stringify(output)).toContain("execute_tool");
    expect(output).not.toHaveProperty("nextCursor");
    expect(cap.calls.map((call) => call.url)).toEqual([
      "https://api.example/api/sessions/ses_cli/otel?signal=traces",
      "https://api.example/api/sessions/ses_cli/otel?signal=traces&cursor=page-2"
    ]);
  });

  it("selects OTLP logs explicitly", async () => {
    const cap = makeIo({
      argv: ["otel", "ses_cli", "--signal", "logs", "--json", ...COMMON],
      fetchHandler: () => Response.json({ resourceLogs: [] })
    });

    await executeCli(cap.io);

    expect(cap.exitCode).toBe(0);
    expect(JSON.parse(cap.stdout)).toEqual({ resourceLogs: [] });
    expect(cap.calls[0]?.url).toBe("https://api.example/api/sessions/ses_cli/otel?signal=logs");
  });
});
