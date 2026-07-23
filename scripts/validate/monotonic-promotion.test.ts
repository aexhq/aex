import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "bun:test";
// @ts-expect-error JS release helper is validated directly.
import { assertMonotonicPromotion, assertPromotionInRepository } from "../cicd/assert-monotonic-promotion.mjs";

const candidateSha = "0123456789abcdef0123456789abcdef01234567";
const mainSha = "f".repeat(40);
const latestSha = "1".repeat(40);
const canarySha = "2".repeat(40);

function attestedTags(overrides: Record<string, unknown> = {}) {
  return [
    {
      tag: "latest",
      version: "0.42.0-canary.80.g111111111111",
      sourceSha: latestSha,
      sourceIsCandidateAncestor: true
    },
    {
      tag: "canary",
      version: "0.42.0-canary.81.g222222222222",
      sourceSha: canarySha,
      sourceIsCandidateAncestor: true
    }
  ].map((tagged) => ({ ...tagged, ...overrides }));
}

function createAncestryRepository() {
  const repository = mkdtempSync(join(tmpdir(), "aex-promotion-ancestry-"));
  execFileSync("git", ["init", "--bare", "--quiet"], { cwd: repository });

  const commits = [
    { ref: "first", mark: 1, parent: null, content: "first" },
    { ref: "second", mark: 2, parent: 1, content: "second" },
    { ref: "newer", mark: 3, parent: 2, content: "newer" },
    { ref: "divergent", mark: 4, parent: 2, content: "divergent" }
  ];
  const fastImport = commits.map(({ ref, mark, parent, content }) => [
    `commit refs/heads/${ref}`,
    `mark :${mark}`,
    `author aex release test <release-test@aex.dev> ${1_700_000_000 + mark} +0000`,
    `committer aex release test <release-test@aex.dev> ${1_700_000_000 + mark} +0000`,
    "data <<COMMIT_MESSAGE",
    content,
    "COMMIT_MESSAGE",
    ...(parent === null ? [] : [`from :${parent}`]),
    "M 100644 inline source.txt",
    "data <<FILE_CONTENT",
    content,
    "FILE_CONTENT",
    ""
  ].join("\n")).join("\n");
  execFileSync("git", ["fast-import", "--quiet"], { cwd: repository, input: fastImport });

  const shas = execFileSync("git", [
    "rev-parse",
    "refs/heads/first",
    "refs/heads/second",
    "refs/heads/newer",
    "refs/heads/divergent"
  ], { cwd: repository, encoding: "utf8" }).trim().split(/\r?\n/);
  if (shas.length !== 4 || shas.some((sha) => !/^[0-9a-f]{40}$/.test(sha))) {
    throw new Error("failed to create the deterministic Git ancestry fixture");
  }
  const [first, second, newer, divergent] = shas as [string, string, string, string];
  return { repository, first, second, newer, divergent };
}

describe("monotonic promotion guard", () => {
  it("accepts a tested main descendant of the current latest and canary sources", () => {
    expect(assertMonotonicPromotion({
      candidateVersion: "0.42.0-canary.82.g0123456789ab",
      candidateSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      taggedReleases: attestedTags()
    })).toMatchObject({
      candidateVersion: "0.42.0-canary.82.g0123456789ab",
      candidateSha,
      currentMainSha: mainSha
    });
  });

  it("rejects a candidate that is not in current main history", () => {
    expect(() => assertMonotonicPromotion({
      candidateVersion: "0.42.0-canary.82.g0123456789ab",
      candidateSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: false,
      taggedReleases: attestedTags()
    })).toThrow(/not an ancestor/);
  });

  it("rejects source rollback even when candidate semver advances", () => {
    expect(() => assertMonotonicPromotion({
      candidateVersion: "0.43.0-canary.90.g0123456789ab",
      candidateSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      taggedReleases: attestedTags({ sourceIsCandidateAncestor: false })
    })).toThrow(/source .* behind the candidate/);
  });

  it("rejects promotion behind either current registry tag", () => {
    expect(() => assertMonotonicPromotion({
      candidateVersion: "0.42.0-canary.81.g0123456789ab",
      candidateSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      taggedReleases: attestedTags()
    })).toThrow(/older than current canary/);
  });

  it("permits only the explicit pre-attestation migration window", () => {
    expect(assertMonotonicPromotion({
      candidateVersion: "0.42.0-canary.82.g0123456789ab",
      candidateSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      taggedReleases: [
        { tag: "latest", version: "0.41.5", sourceSha: "" },
        { tag: "canary", version: "0.41.5", sourceSha: "" }
      ]
    }).taggedReleases).toEqual([
      expect.objectContaining({ tag: "latest", sourceProof: "legacy-unattested" }),
      expect.objectContaining({ tag: "canary", sourceProof: "legacy-unattested" })
    ]);

    expect(() => assertMonotonicPromotion({
      candidateVersion: "0.43.0-canary.90.g0123456789ab",
      candidateSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      taggedReleases: [
        { tag: "latest", version: "0.42.0-canary.82.g0123456789ab", sourceSha: "" },
        { tag: "canary", version: "0.42.0-canary.82.g0123456789ab", sourceSha: candidateSha }
      ]
    })).toThrow(/missing source attestation/);
  });

  it("fails closed on malformed or contradictory registry source data", () => {
    expect(() => assertMonotonicPromotion({
      candidateVersion: "0.42.0-canary.82.g0123456789ab",
      candidateSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      taggedReleases: attestedTags({ sourceSha: "not-a-sha" })
    })).toThrow(/invalid .* source SHA/);

    expect(() => assertMonotonicPromotion({
      candidateVersion: "0.42.0-canary.82.g0123456789ab",
      candidateSha,
      currentMainSha: mainSha,
      candidateIsMainAncestor: true,
      taggedReleases: attestedTags({ sourceSha: "3".repeat(40) })
    })).toThrow(/does not match source attestation/);
  });

  // This intentionally spawns several Git subprocesses. On shared/self-hosted
  // runners the fixture can be CPU/IO delayed even though the ancestry check is
  // deterministic; bound the test for that host load without retrying it.
  it("checks real Git ancestry rather than trusting a caller-provided version order", () => {
    const { repository, first, second, newer, divergent } = createAncestryRepository();
    try {
      expect(() => assertPromotionInRepository({
        repository,
        candidateVersion: "0.43.0-canary.99.g000000000000",
        candidateSha: divergent,
        currentMainSha: newer,
        taggedReleases: [
          { tag: "latest", version: "0.42.0", sourceSha: first },
          { tag: "canary", version: "0.42.1-canary.98.g" + newer.slice(0, 12), sourceSha: newer }
        ]
      })).toThrow(/current main|current canary/);

      expect(assertPromotionInRepository({
        repository,
        candidateVersion: "0.42.1-canary.99.g" + newer.slice(0, 12),
        candidateSha: newer,
        currentMainSha: newer,
        taggedReleases: [
          { tag: "latest", version: "0.42.0", sourceSha: first },
          { tag: "canary", version: "0.42.1-canary.98.g" + second.slice(0, 12), sourceSha: second }
        ]
      }).candidateSha).toBe(newer);
    } finally {
      rmSync(repository, { recursive: true, force: true });
    }
  }, 20_000);
});
