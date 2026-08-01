import { describe, expect, test } from "bun:test";

import {
  AexConfigError,
  AexStreamProtocolError,
  Download,
  Page,
  parseNdjsonFrames,
  planDownloadRanges,
} from "../../src/index.js";

describe("pagination", () => {
  test("visits arbitrary page splits in order", async () => {
    const page = new Page([1, 2], "cur_1", async (cursor) => {
      if (cursor === "cur_1") return new Page([3], "cur_2", async () => new Page([4]));
      throw new Error("unexpected cursor");
    });
    const values: number[] = [];
    for await (const value of page) values.push(value);
    expect(values).toEqual([1, 2, 3, 4]);
  });

  test("rejects a repeated cursor", async () => {
    const page = new Page([1], "cur_repeat", async () => new Page([2], "cur_repeat"));
    const consume = async () => {
      for await (const _value of page) void _value;
    };
    await expect(consume()).rejects.toBeInstanceOf(AexStreamProtocolError);
  });
});

describe("downloads and NDJSON", () => {
  test("plans half-open ranges and verifies length", async () => {
    expect(planDownloadRanges(0)).toEqual([]);
    expect(planDownloadRanges(10, { start: 2, endExclusive: 6 })).toEqual([
      { start: 2, endExclusive: 6 },
    ]);
    const download = new Download(
      {
        url: "https://download.example/object",
        expiresAt: "2999-01-01T00:00:00Z",
        sizeBytes: 3,
        authorizedBytes: 3,
        measurementId: "mea_1",
        sha256: "039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81",
      },
      async () => new Response(new Uint8Array([1, 2, 3])),
    );
    expect(await download.bytes()).toEqual(new Uint8Array([1, 2, 3]));
  });

  test("rejects grant under-delivery", async () => {
    const download = new Download(
      {
        url: "https://download.example/object",
        expiresAt: "2999-01-01T00:00:00Z",
        sizeBytes: 3,
        authorizedBytes: 3,
        measurementId: "mea_1",
        sha256: "unused-for-range",
      },
      async () => new Response(new Uint8Array([1, 2])),
    );
    await expect(download.bytes()).rejects.toBeInstanceOf(AexConfigError);
  });

  test("parses frames across arbitrary chunks and never advances over a partial tail", async () => {
    const chunks = ["{\"type\":\"cursor\",", "\"cursor\":\"cur_1\"}\n{\"type\":\"rotate\"" ];
    const frames = await parseNdjsonFrames(chunks);
    expect(frames).toEqual([{ type: "cursor", cursor: "cur_1" }]);
  });
});
