import { describe, expect, it } from "bun:test";
import {
  BUILTIN_TOOL_NAMES,
  BuiltinTools,
  DEFAULT_BUILTIN_TOOLS,
  resolveBuiltinToolNames,
  MODEL_SLUG_PATTERN,
  isModelSlug,
  parseModelSlug
} from "../src/index.js";
import { parseSessionSubmissionRequest } from "../src/internal.js";

const MODEL = "anthropic/claude-haiku-4-5";

function baseRequest() {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    submission: {
      model: MODEL,
      prompt: ["hello"],
      assets: { files: [], skills: [], tools: [], instructions: [] },
      builtinTools: "default",
      mcpServers: []
    },
    secrets: {}
  };
}

describe("submission parser - managed gateway model slug", () => {
  it("accepts a well-formed creator/model slug and preserves it verbatim", () => {
    const parsed = parseSessionSubmissionRequest(baseRequest());
    expect(parsed.submission.model).toBe(MODEL);
  });

  it("accepts arbitrary gateway slugs (open catalog, no closed list)", () => {
    for (const slug of [
      "deepseek/deepseek-v4-flash",
      "openai/gpt-4.1",
      "x-ai/grok-2",
      "google/gemini-2.5-flash",
      "anthropic/claude-sonnet-4-6:beta"
    ]) {
      const base = baseRequest();
      const parsed = parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, model: slug }
      });
      expect(parsed.submission.model).toBe(slug);
    }
  });

  it("rejects a bare model id with no creator prefix", () => {
    const base = baseRequest();
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, model: "claude-haiku-4-5" }
      })
    ).toThrow(/submission\.model must be a gateway model slug/);
  });

  it("rejects an uppercase creator segment", () => {
    const base = baseRequest();
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, model: "Anthropic/claude-haiku-4-5" }
      })
    ).toThrow(/submission\.model must be a gateway model slug/);
  });

  it("rejects a missing model", () => {
    const base = baseRequest();
    const { model: _drop, ...submission } = base.submission;
    expect(() =>
      parseSessionSubmissionRequest({ ...base, submission })
    ).toThrow(/submission\.model must be a non-empty gateway model slug/);
  });
});

describe("submission parser - provider and apiKeys are removed from the contract", () => {
  it("rejects a top-level provider field", () => {
    expect(() =>
      parseSessionSubmissionRequest({ ...baseRequest(), provider: "anthropic" })
    ).toThrow(/submission\.provider is not an allowed field/);
  });

  it("rejects secrets.apiKeys (managed keys only)", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...baseRequest(),
        secrets: { apiKeys: { anthropic: "sk-ant" } }
      })
    ).toThrow(/secrets\.apiKeys is not an allowed field; permitted: mcpServers, envSecrets/);
  });

  it("accepts an empty secrets bundle (a run needs no provider key)", () => {
    const parsed = parseSessionSubmissionRequest(baseRequest());
    expect(parsed.secrets).toEqual({});
  });

  it("rejects unknown sibling keys inside the secrets bundle", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...baseRequest(),
        secrets: { openai: { apiKey: "sk-openai-x" } }
      })
    ).toThrow(
      /secrets\.openai is not an allowed field; permitted: mcpServers, envSecrets/
    );
  });
});

describe("parseModelSlug / isModelSlug", () => {
  it("MODEL_SLUG_PATTERN matches creator/model shapes", () => {
    expect(MODEL_SLUG_PATTERN.test("anthropic/claude-haiku-4-5")).toBe(true);
    expect(MODEL_SLUG_PATTERN.test("openai/gpt-4.1")).toBe(true);
    expect(MODEL_SLUG_PATTERN.test("no-slash")).toBe(false);
    expect(MODEL_SLUG_PATTERN.test("UPPER/model")).toBe(false);
  });

  it("isModelSlug narrows well-formed strings", () => {
    expect(isModelSlug("deepseek/deepseek-v4-flash")).toBe(true);
    expect(isModelSlug("deepseek")).toBe(false);
    expect(isModelSlug(123)).toBe(false);
  });

  it("parseModelSlug returns the slug or throws with field context", () => {
    expect(parseModelSlug("anthropic/claude-haiku-4-5")).toBe("anthropic/claude-haiku-4-5");
    expect(() => parseModelSlug("bogus", "my.field")).toThrow(/my\.field must be a gateway model slug/);
  });
});

describe("submission parser - removed choice fields", () => {
  // `parentSessionId` was the legacy lineage field for API-submitted child sessions.
  // Child sessions are now in-brain threads, so the top-level submit contract no
  // longer accepts it — the strict allow-list rejects it like any unknown field.
  it.each(["runtime", "region", "credentialMode", "parentSessionId", "provider"] as const)(
    "rejects top-level %s",
    (field) => {
      expect(() =>
        parseSessionSubmissionRequest({ ...baseRequest(), [field]: "managed" })
      ).toThrow(new RegExp(`submission\\.${field} is not an allowed field`));
    }
  );
});

describe("builtin tool exports", () => {
  it("BUILTIN_TOOL_NAMES is the closed builtin tool-name set (HANDS_TOOLS order)", () => {
    expect([...BUILTIN_TOOL_NAMES]).toEqual([
      "bash",
      "read_file",
      "write_file",
      "edit_file",
      "grep",
      "glob",
      "head",
      "tail",
      "todo_write",
      "subagent",
      "subagent_result",
      "web_fetch",
      "web_search",
      "bash_output",
      "bash_kill",
      "code_execution",
      "wait",
      "git",
      "ls",
      "stat",
      "wc"
    ]);
  });

  it("BuiltinTools maps each tool name to itself and never drifts from BUILTIN_TOOL_NAMES", () => {
    expect(Object.values(BuiltinTools)).toEqual([...BUILTIN_TOOL_NAMES]);
    for (const name of BUILTIN_TOOL_NAMES) {
      expect(BuiltinTools[name]).toBe(name);
    }
  });

  it("DEFAULT_BUILTIN_TOOLS is the complete closed builtin set", () => {
    expect([...DEFAULT_BUILTIN_TOOLS]).toEqual([...BUILTIN_TOOL_NAMES]);
  });

  it("resolveBuiltinToolNames handles default, none, explicit selection, dedupe, and rejection", () => {
    expect(resolveBuiltinToolNames()).toEqual([...DEFAULT_BUILTIN_TOOLS]);
    expect(resolveBuiltinToolNames("default")).toEqual([...DEFAULT_BUILTIN_TOOLS]);
    expect(resolveBuiltinToolNames("none")).toEqual([]);
    for (const name of BUILTIN_TOOL_NAMES) {
      expect(resolveBuiltinToolNames([name]), name).toEqual([name]);
    }
    expect(resolveBuiltinToolNames([BuiltinTools.git, BuiltinTools.bash, BuiltinTools.git])).toEqual(
      BUILTIN_TOOL_NAMES.filter((name) => name === BuiltinTools.bash || name === BuiltinTools.git)
    );
    expect(() => resolveBuiltinToolNames(["nope" as never])).toThrow(/is not a builtin tool/);
    expect(() => resolveBuiltinToolNames(["notebook_edit" as never])).toThrow(/is not a builtin tool/);
  });
});

describe("submission parser - builtinTools", () => {
  it("normalizes an omitted selection to default", () => {
    const base = baseRequest();
    const { builtinTools: _selection, ...submission } = base.submission;
    const parsed = parseSessionSubmissionRequest({ ...base, submission });
    expect(parsed.submission.builtinTools).toBe("default");
  });

  it("accepts none and explicit selections", () => {
    const base = baseRequest();
    const none = parseSessionSubmissionRequest({
      ...base,
      submission: { ...base.submission, builtinTools: "none" }
    });
    expect(none.submission.builtinTools).toBe("none");
    const selected = parseSessionSubmissionRequest({
      ...base,
      submission: { ...base.submission, builtinTools: [BuiltinTools.wait, BuiltinTools.git] }
    });
    expect(selected.submission.builtinTools).toEqual(["wait", "git"]);
  });

  it("dedupes explicit selections in canonical order", () => {
    const base = baseRequest();
    const parsed = parseSessionSubmissionRequest({
      ...base,
      submission: {
        ...base.submission,
        builtinTools: [BuiltinTools.web_search, BuiltinTools.web_search, BuiltinTools.git]
      }
    });
    expect(parsed.submission.builtinTools).toEqual(["web_search", "git"]);
  });

  it("rejects names outside the closed builtin-tool set", () => {
    const base = baseRequest();
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, builtinTools: ["not_a_tool"] }
      })
    ).toThrow(/not a builtin tool/);
  });
});

describe("submission parser - outputMode", () => {
  it("accepts 'buffered' and 'stream' for any model (streaming is unconditional)", () => {
    for (const mode of ["buffered", "stream"] as const) {
      const req = baseRequest();
      const parsed = parseSessionSubmissionRequest({
        ...req,
        submission: { ...req.submission, outputMode: mode }
      });
      expect(parsed.submission.outputMode).toBe(mode);
    }
  });

  it("omits outputMode when not provided", () => {
    const parsed = parseSessionSubmissionRequest(baseRequest());
    expect("outputMode" in parsed.submission).toBe(false);
  });

  it("rejects an unknown outputMode", () => {
    const req = baseRequest();
    expect(() =>
      parseSessionSubmissionRequest({ ...req, submission: { ...req.submission, outputMode: "fast" } })
    ).toThrow(/outputMode/);
  });
});

describe("submission parser - removed postHook", () => {
  it("rejects postHook on the public submission surface", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...baseRequest(),
        postHook: { command: "bun test" }
      })
    ).toThrow(/submission\.postHook is not an allowed field/);
  });
});
