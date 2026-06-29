import { describe, expect, it, vi } from "vitest";
import { AexApiError, operations, type HttpClient } from "../src/index.js";

describe("operations.createSkillBundleDirect", () => {
  it("treats presign_unconfigured as terminal and does not POST a bundle to the API", async () => {
    const calls: string[] = [];
    const http = {
      request: vi.fn(async (path: string) => {
        calls.push(path);
        if (path === "/api/skills/presign") {
          throw new AexApiError(503, "object storage S3 creds not configured", {
            ok: false,
            code: "presign_unconfigured"
          });
        }
        throw new Error(`unexpected request: ${path}`);
      })
    } as unknown as HttpClient;
    const fetchImpl = vi.fn(async () => ({ ok: true, status: 200, text: async () => "" }));

    await expect(
      operations.createSkillBundleDirect(http, fetchImpl, {
        name: "rules",
        body: new Uint8Array([1, 2, 3]),
        contentHash: `sha256:${"a".repeat(64)}`,
        manifest: [{ path: "SKILL.md", size: 1 }]
      })
    ).rejects.toThrow(/object storage S3 creds not configured/);

    expect(calls).toEqual(["/api/skills/presign"]);
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});
