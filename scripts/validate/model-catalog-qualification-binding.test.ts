import { afterEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync
} from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

const root = resolve(import.meta.dir, "../..");
const script = resolve(root, "scripts/cicd/model-catalog-qualification-binding.mjs");
const temporaryDirectories: string[] = [];
const sourceSha = "1".repeat(40);
const adapterSource = `blake3:${"2".repeat(64)}`;
const entryDigest = `sha256:${"3".repeat(64)}`;
const tokenizerDigest = "sha256:ecb6f9fc369894346f0511f4074ca75cee5cd5f3b06d02f1ba35fcd39f8e121d";
const templateDigest = "sha256:3a3036eeb96ca0e48565fc1f19b4d7d3fa17ada32bb1b3d5ed539ae74cf22923";
const probes = Array.from({ length: 23 }, (_, index) => `p-${String(index + 1).padStart(2, "0")}`);
const testTimeoutMs = 15_000;

function canonicalValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonicalValue);
  }
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => [key, canonicalValue(item)])
    );
  }
  return value;
}

function canonicalBytes(value: unknown): Buffer {
  return Buffer.from(JSON.stringify(canonicalValue(value)));
}

function digest(bytes: Uint8Array): string {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function run(...arguments_: string[]): ReturnType<typeof Bun.spawnSync> {
  return Bun.spawnSync([process.execPath, script, ...arguments_], {
    cwd: root,
    stdout: "pipe",
    stderr: "pipe"
  });
}

function errorText(result: ReturnType<typeof Bun.spawnSync>): string {
  return result.stderr?.toString() ?? "";
}

function fixture(): { directory: string } {
  const base = mkdtempSync(join(tmpdir(), "aex-catalog-binding-"));
  temporaryDirectories.push(base);
  const directory = join(base, "qualification");
  mkdirSync(directory);

  const aggregateUsage = {
    cache_read_input_tokens: 0,
    cache_write_input_tokens: 0,
    completeness: { kind: "absent" },
    input_tokens: 1,
    output_tokens: 1,
    provider_total_tokens: null,
    reasoning_tokens: 0,
    tool_use_prompt_tokens: 0
  };
  const evidenceProbes = probes.map((probe) => ({
    duration_ms: 1,
    observed: [],
    outcome: { outcome: "pass" },
    probe
  }));
  const evidence = canonicalBytes({
    aggregate_usage: aggregateUsage,
    authority: {
      chat_template_sha256: templateDigest,
      publisher: "aex-catalog-prd",
      tokenizer_sha256: tokenizerDigest
    },
    bounds: {
      approved_budget_micro_usd: 250000,
      approved_runtime_seconds: 1800,
      maximum_cost_micro_usd: 200000
    },
    probes: evidenceProbes,
    run: {
      attempt: 2,
      id: 1234,
      issued_at: "2027-01-15T08:00:01.000Z",
      plane: "dev",
      ran_at: "2027-01-15T08:00:00.000Z",
      region: "eu-west-1"
    },
    schema: "aex.model-catalog-qualification-evidence.v1",
    source: { repository: "aexhq/aex", sha: sourceSha },
    target: { model: "deepseek-v4-flash", provider: "deepseek" }
  });
  const receipt = {
    adapter_source: adapterSource,
    catalog_entry_digest: entryDigest,
    evidence_digest: digest(evidence),
    expires_at: "2027-01-22T08:00:00.000Z",
    plane: "dev",
    probe_suite_revision: 1,
    provider_request_ids: [],
    ran_at: "2027-01-15T08:00:00.000Z",
    receipt_id: "018f0000-0000-7000-8000-000000000001",
    region: "eu-west-1",
    results: probes.map((probe) => ({
      duration_ms: 1,
      observed: [],
      outcome: "pass",
      probe
    })),
    tokens_spent: aggregateUsage
  };
  writeFileSync(join(directory, "evidence.json"), evidence);
  writeFileSync(join(directory, "receipt.json"), canonicalBytes(receipt));
  return { directory };
}

function create(directory: string): ReturnType<typeof Bun.spawnSync> {
  return run(
    "create",
    "--directory", directory,
    "--repository", "aexhq/aex",
    "--source-sha", sourceSha,
    "--source-ref", "refs/heads/main",
    "--workflow-path", ".github/workflows/model-catalog-qualify.yml",
    "--workflow-ref", "aexhq/aex/.github/workflows/model-catalog-qualify.yml@refs/heads/main",
    "--workflow-sha", sourceSha,
    "--run-id", "1234",
    "--run-attempt", "2",
    "--maximum-budget-micro-usd", "250000",
    "--maximum-runtime-seconds", "1800"
  );
}

function rewriteEvidence(
  directory: string,
  mutate: (evidence: Record<string, any>) => void
): void {
  const evidencePath = join(directory, "evidence.json");
  const receiptPath = join(directory, "receipt.json");
  const evidence = JSON.parse(readFileSync(evidencePath, "utf8"));
  mutate(evidence);
  const evidenceBytes = canonicalBytes(evidence);
  writeFileSync(evidencePath, evidenceBytes);
  const receipt = JSON.parse(readFileSync(receiptPath, "utf8"));
  receipt.evidence_digest = digest(evidenceBytes);
  writeFileSync(receiptPath, canonicalBytes(receipt));
}

function verify(directory: string): ReturnType<typeof Bun.spawnSync> {
  const binding = readFileSync(join(directory, "qualification-binding.json"));
  return run(
    "verify",
    "--directory", directory,
    "--binding-sha256", digest(binding),
    "--expected-repository", "aexhq/aex",
    "--expected-source-sha", sourceSha,
    "--expected-run-id", "1234",
    "--expected-run-attempt", "2"
  );
}

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("model-catalog qualification binding", () => {
  test("creates and verifies the exact canonical Rust-serde qualification relationship", () => {
    const { directory } = fixture();
    const created = create(directory);
    expect(created.exitCode, errorText(created)).toBe(0);
    expect(readFileSync(join(directory, "qualification-binding.json"))).toEqual(
      canonicalBytes(JSON.parse(readFileSync(join(directory, "qualification-binding.json"), "utf8")))
    );
    const verified = verify(directory);
    expect(verified.exitCode, errorText(verified)).toBe(0);
  }, testTimeoutMs);

  test("rejects evidence tampering even when the changed evidence stays canonical", () => {
    const { directory } = fixture();
    expect(create(directory).exitCode).toBe(0);
    writeFileSync(join(directory, "evidence.json"), canonicalBytes({ changed: true }));
    const verified = verify(directory);
    expect(verified.exitCode).not.toBe(0);
    expect(errorText(verified)).toContain("evidence_digest");
  }, testTimeoutMs);

  test("rejects evidence whose source or bounds differ from protected workflow inputs", () => {
    const { directory } = fixture();
    rewriteEvidence(directory, (evidence) => {
      evidence.source.sha = "4".repeat(40);
      evidence.bounds.approved_runtime_seconds = 1799;
    });
    const created = create(directory);
    expect(created.exitCode).not.toBe(0);
    expect(errorText(created)).toContain("protected workflow source");
  }, testTimeoutMs);

  test("rejects a partial or reordered evidence probe inventory", () => {
    const { directory } = fixture();
    rewriteEvidence(directory, (evidence) => {
      evidence.probes = evidence.probes.slice(1);
    });
    const created = create(directory);
    expect(created.exitCode).not.toBe(0);
    expect(errorText(created)).toContain("complete 23-probe inventory");
  }, testTimeoutMs);

  test("requires the qualifier's incomplete whole-matrix usage representation", () => {
    const exact = fixture();
    rewriteEvidence(exact.directory, (evidence) => {
      evidence.aggregate_usage.completeness.kind = "exact";
    });
    const exactResult = create(exact.directory);
    expect(exactResult.exitCode).not.toBe(0);
    expect(errorText(exactResult)).toContain("incomplete matrix contract");

    const providerTotal = fixture();
    rewriteEvidence(providerTotal.directory, (evidence) => {
      evidence.aggregate_usage.provider_total_tokens = 2;
    });
    const providerTotalResult = create(providerTotal.directory);
    expect(providerTotalResult.exitCode).not.toBe(0);
    expect(errorText(providerTotalResult)).toContain("incomplete matrix contract");
  }, testTimeoutMs);

  test("rejects files outside the exact three-file monitoring allowlist", () => {
    const { directory } = fixture();
    expect(create(directory).exitCode).toBe(0);
    writeFileSync(join(directory, "extra.json"), "{}");
    const verified = verify(directory);
    expect(verified.exitCode).not.toBe(0);
    expect(errorText(verified)).toContain("allowlist mismatch");
  }, testTimeoutMs);

  test("rejects a noncanonical receipt before creating a binding", () => {
    const { directory } = fixture();
    const receipt = JSON.parse(readFileSync(join(directory, "receipt.json"), "utf8"));
    writeFileSync(join(directory, "receipt.json"), JSON.stringify(receipt, null, 2));
    const created = create(directory);
    expect(created.exitCode).not.toBe(0);
    expect(errorText(created)).toContain("receipt.json is not canonical JSON");
  }, testTimeoutMs);

  test("rejects a receipt that does not identify the evidence bytes", () => {
    const { directory } = fixture();
    const path = join(directory, "receipt.json");
    const receipt = JSON.parse(readFileSync(path, "utf8"));
    receipt.evidence_digest = `sha256:${"9".repeat(64)}`;
    writeFileSync(path, canonicalBytes(receipt));
    const created = create(directory);
    expect(created.exitCode).not.toBe(0);
    expect(errorText(created)).toContain("evidence_digest");
  }, testTimeoutMs);

  test("rejects the Rust enum's non-wire spelling", () => {
    const { directory } = fixture();
    rewriteEvidence(directory, (evidence) => {
      evidence.probes[0].outcome.outcome = "Pass";
    });
    const created = create(directory);
    expect(created.exitCode).not.toBe(0);
    expect(errorText(created)).toContain("closed schema");
  }, testTimeoutMs);
});
