import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";

import {
  CANARY_MANIFEST_SCHEMA,
  buildCanaryManifest,
  parseCanaryManifest
} from "../cicd/canary-manifest.mjs";
import { readModuleGraph } from "../cicd/public-module-graph.mjs";
import { selectManifestEntry, verifyCanarySelection } from "../cicd/verify-canary-selection.mjs";

const repoRoot = resolve(import.meta.dir, "..", "..");
const SHA = "0123456789abcdef0123456789abcdef01234567";
const OTHER_SHA = "89abcdef0123456789abcdef0123456789abcdef";
const INTEGRITY = "sha512-qyF1qjoLAp+Epu6azUW9PIhg3w7FWjC1IS5B4Gxsl/NpZtgEMccpY+iFPXrXVygFTLeL7lkMF+EcONMU4Szn3g==";
const OTHER_INTEGRITY = "sha512-AAF1qjoLAp+Epu6azUW9PIhg3w7FWjC1IS5B4Gxsl/NpZtgEMccpY+iFPXrXVygFTLeL7lkMF+EcONMU4Szn3g==";

function build(entries: readonly { module: string; name: string; version: string }[]) {
  return buildCanaryManifest({
    repoRoot,
    repository: "aexhq/aex",
    sourceSha: SHA,
    workflowRunId: "12345",
    generatedAt: "2026-07-28T00:00:00.000Z",
    entries: entries.map((entry) => ({
      ...entry,
      integrity: INTEGRITY,
      sourceTag: `canary/${entry.module}/${entry.version}`
    }))
  });
}

describe("the canary manifest is release identity, not lockfile data", () => {
  it("records version, integrity, source commit, and source tag for every package", () => {
    const manifest = build([{ module: "sdk", name: "@aexhq/sdk", version: "0.46.4-canary" }]);
    expect(manifest.schema).toBe(CANARY_MANIFEST_SCHEMA);
    expect(manifest.packages).toHaveLength(1);
    const entry = manifest.packages[0];
    expect(entry).toMatchObject({
      module: "sdk",
      name: "@aexhq/sdk",
      version: "0.46.4-canary",
      npmDistTag: "canary",
      integrity: INTEGRITY,
      sourceRepository: "aexhq/aex",
      sourceSha: SHA
    });
  });

  it("pins an upstream published in the same release to that release's canary", () => {
    const manifest = build([
      { module: "sdk", name: "@aexhq/sdk", version: "0.46.4-canary" },
      { module: "contracts", name: "@aexhq/contracts", version: "0.34.0-canary" }
    ]);
    const sdk = manifest.packages.find((entry) => entry.module === "sdk")!;
    // The SDK embeds contracts AND the CLI bundle at build time, so a canary
    // built against a different one of either is a different artifact at the
    // same commit. `@aexhq/cli` was not published in this release, so it pins to
    // the version the workspace built against.
    expect(sdk.upstream).toEqual({
      "@aexhq/cli": readModuleGraph(repoRoot).byId.get("cli")!.version,
      "@aexhq/contracts": "0.34.0-canary"
    });
  });

  it("pins an unaffected upstream to the version the workspace built against", () => {
    const manifest = build([{ module: "sdk", name: "@aexhq/sdk", version: "0.46.4-canary" }]);
    const sdk = manifest.packages.find((entry) => entry.module === "sdk")!;
    expect(sdk.upstream["@aexhq/contracts"]).toMatch(/^\d+\.\d+\.\d+$/);
    expect(sdk.upstream["@aexhq/contracts"]).not.toMatch(/-canary$/);
  });

  it("sorts packages so two runs of the same release produce the same bytes", () => {
    const forward = build([
      { module: "sdk", name: "@aexhq/sdk", version: "0.46.4-canary" },
      { module: "cli", name: "@aexhq/cli", version: "0.34.0-canary" }
    ]);
    const reverse = build([
      { module: "cli", name: "@aexhq/cli", version: "0.34.0-canary" },
      { module: "sdk", name: "@aexhq/sdk", version: "0.46.4-canary" }
    ]);
    expect(JSON.stringify(forward)).toBe(JSON.stringify(reverse));
  });

  it("refuses a release entry that disagrees with the workspace", () => {
    expect(() =>
      buildCanaryManifest({
        repoRoot,
        repository: "aexhq/aex",
        sourceSha: SHA,
        workflowRunId: "1",
        generatedAt: "2026-07-28T00:00:00.000Z",
        entries: [{ module: "sdk", name: "@aexhq/contracts", version: "1.0.0-canary", integrity: INTEGRITY, sourceTag: "t" }]
      })
    ).toThrow(/claims package @aexhq\/contracts, workspace says @aexhq\/sdk/);
  });
});

describe("the manifest parser fails closed on anything a consumer could not verify", () => {
  const valid = () => JSON.parse(JSON.stringify(build([{ module: "sdk", name: "@aexhq/sdk", version: "0.46.4-canary" }])));

  it("accepts the manifest it produced", () => {
    expect(() => parseCanaryManifest(valid())).not.toThrow();
  });

  it("rejects a range or a dist-tag where an exact version belongs", () => {
    for (const version of ["^0.46.4", "~0.46.4", "latest", "canary", "0.46.x", ""]) {
      const manifest = valid();
      manifest.packages[0].version = version;
      expect(() => parseCanaryManifest(manifest), version).toThrow();
    }
  });

  it("rejects a missing or malformed integrity value", () => {
    for (const integrity of [undefined, "", "sha1-abc", "not-an-integrity"]) {
      const manifest = valid();
      manifest.packages[0].integrity = integrity;
      expect(() => parseCanaryManifest(manifest), String(integrity)).toThrow();
    }
  });

  it("rejects a package built from a different commit than the manifest", () => {
    const manifest = valid();
    manifest.packages[0].sourceSha = OTHER_SHA;
    expect(() => parseCanaryManifest(manifest)).toThrow(/differs from manifest.sourceSha/);
  });

  it("rejects a dist-tag other than canary", () => {
    const manifest = valid();
    manifest.packages[0].npmDistTag = "latest";
    expect(() => parseCanaryManifest(manifest)).toThrow(/npmDistTag/);
  });

  it("rejects a duplicated package", () => {
    const manifest = valid();
    manifest.packages.push({ ...manifest.packages[0] });
    expect(() => parseCanaryManifest(manifest)).toThrow(/duplicate package/);
  });

  it("rejects an upstream pin that contradicts what this release published", () => {
    const manifest = JSON.parse(
      JSON.stringify(
        build([
          { module: "sdk", name: "@aexhq/sdk", version: "0.46.4-canary" },
          { module: "contracts", name: "@aexhq/contracts", version: "0.34.0-canary" }
        ])
      )
    );
    manifest.packages.find((entry: { module: string }) => entry.module === "sdk")!.upstream["@aexhq/contracts"] =
      "0.33.0";
    expect(() => parseCanaryManifest(manifest)).toThrow(/this release published @aexhq\/contracts@0.34.0-canary/);
  });

  it("rejects an empty package set, a wrong schema, and a non-canonical timestamp", () => {
    const empty = valid();
    empty.packages = [];
    expect(() => parseCanaryManifest(empty)).toThrow(/must not be empty/);

    const wrongSchema = valid();
    wrongSchema.schema = "aex.public-canary-manifest.v2";
    expect(() => parseCanaryManifest(wrongSchema)).toThrow(/schema/);

    const wrongTime = valid();
    wrongTime.generatedAt = "2026-07-28";
    expect(() => parseCanaryManifest(wrongTime)).toThrow(/ISO-8601/);
  });
});

describe("promotion verifies the registry, not only the manifest", () => {
  const manifest = () => JSON.parse(JSON.stringify(build([{ module: "sdk", name: "@aexhq/sdk", version: "0.46.4-canary" }])));

  function registry(packument: unknown) {
    return async () => ({ ok: true, status: 200, json: async () => packument });
  }

  it("selects the manifest entry by module and exact version", () => {
    const parsed = parseCanaryManifest(manifest());
    expect(selectManifestEntry(parsed, "sdk", "0.46.4-canary").name).toBe("@aexhq/sdk");
    expect(() => selectManifestEntry(parsed, "cli", "0.46.4-canary")).toThrow(/has no module cli/);
    expect(() => selectManifestEntry(parsed, "sdk", "0.46.5-canary")).toThrow(/not 0.46.5-canary/);
  });

  it("accepts a selection whose registry integrity matches", async () => {
    const verified = await verifyCanarySelection(
      { manifest: manifest(), module: "sdk", version: "0.46.4-canary", distTag: "latest" },
      {
        fetch: registry({
          "dist-tags": { canary: "0.46.4-canary", latest: "0.46.3" },
          versions: { "0.46.4-canary": { dist: { integrity: INTEGRITY } } }
        })
      }
    );
    expect(verified.version).toBe("0.46.4-canary");
    expect(verified.sourceSha).toBe(SHA);
  });

  it("refuses a tarball the registry no longer serves at the recorded integrity", async () => {
    await expect(
      verifyCanarySelection(
        { manifest: manifest(), module: "sdk", version: "0.46.4-canary", distTag: "latest" },
        {
          fetch: registry({
            "dist-tags": { canary: "0.46.4-canary" },
            versions: { "0.46.4-canary": { dist: { integrity: OTHER_INTEGRITY } } }
          })
        }
      )
    ).rejects.toThrow(/integrity drift/);
  });

  it("refuses to promote a version that is not the current canary", async () => {
    await expect(
      verifyCanarySelection(
        { manifest: manifest(), module: "sdk", version: "0.46.4-canary", distTag: "latest" },
        {
          fetch: registry({
            "dist-tags": { canary: "0.46.5-canary" },
            versions: { "0.46.4-canary": { dist: { integrity: INTEGRITY } } }
          })
        }
      )
    ).rejects.toThrow(/canary dist-tag is 0.46.5-canary/);
  });

  it("refuses to re-promote a version that already carries the tag", async () => {
    await expect(
      verifyCanarySelection(
        { manifest: manifest(), module: "sdk", version: "0.46.4-canary", distTag: "latest" },
        {
          fetch: registry({
            "dist-tags": { canary: "0.46.4-canary", latest: "0.46.4-canary" },
            versions: { "0.46.4-canary": { dist: { integrity: INTEGRITY } } }
          })
        }
      )
    ).rejects.toThrow(/already carries dist-tag latest/);
  });

  it("refuses a version the registry does not have at all", async () => {
    await expect(
      verifyCanarySelection(
        { manifest: manifest(), module: "sdk", version: "0.46.4-canary", distTag: "latest" },
        { fetch: registry({ "dist-tags": {}, versions: {} }) }
      )
    ).rejects.toThrow(/is not published/);
  });
});
