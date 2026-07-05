import { describe, expect, it, vi } from "vitest";
import { resolve as resolvePath } from "node:path";
import { Aex, type SessionRunOptions } from "@aexhq/sdk";
import { runCli } from "../src/run.js";
import { makeIo } from "./support.js";

const CWD = "/tmp/cli-test";
const abs = (p: string): string => resolvePath(CWD, p);
const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

const SKILL_MD = "---\nname: report-skill\ndescription: A test skill for attach\n---\n# Report skill\n";

describe("aex run --skill/--tool/--agents-md/--file (T6a attach)", () => {
  it("builds an SDK submission carrying every attached asset kind", async () => {
    const submit = vi
      .spyOn(Aex.prototype, "submit")
      .mockResolvedValue({ runId: "r1", session: { record: { id: "s1", status: "running" } } } as never);

    const cap = makeIo({
      argv: [
        "run",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--skill", "@s.md",
        "--tool", "@t.js",
        "--agents-md", "@a.md",
        "--file", "@f.txt",
        "--anthropic-api-key", "sk-ant-1",
        ...COMMON
      ],
      files: {
        [abs("s.md")]: SKILL_MD,
        [abs("t.js")]: "export default async () => ({ ok: true });\n",
        [abs("a.md")]: "# Agent brief\nBe concise.\n",
        [abs("f.txt")]: "reference data\n"
      }
    });

    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(submit).toHaveBeenCalledTimes(1);
    const options = submit.mock.calls[0]![0] as SessionRunOptions;
    expect(options.skills?.length).toBe(1);
    expect(options.tools?.length).toBe(1);
    expect(options.agentsMd?.length).toBe(1);
    expect(options.files?.length).toBe(1);
    // The message (prompt) still rides through.
    expect(options.message).toEqual(["hi"]);
  });

  it("reports a clear error when an attached asset file is missing", async () => {
    const submit = vi.spyOn(Aex.prototype, "submit");
    const cap = makeIo({
      argv: [
        "run",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--file", "@missing.txt",
        "--anthropic-api-key", "sk-ant-1",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("failed to attach asset");
    expect(submit).not.toHaveBeenCalled();
  });
});
