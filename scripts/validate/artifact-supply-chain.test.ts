import { expect, test } from "bun:test";
import { inspectSupplyChain } from "../cicd/artifact-supply-chain.js";

const hex = (byte: string): string => `sha256:${byte.repeat(64)}`;

test("supply-chain evidence binds exact CycloneDX, licenses, database, and severities", () => {
  const output = inspectSupplyChain({
    rawSbom: {
      bomFormat: "CycloneDX",
      specVersion: "1.6",
      components: [
        { name: "aex-wire", version: "0.1.0", licenses: [{ license: { id: "Apache-2.0" } }] },
        { name: "dep", version: "1.0.0", licenses: [{ expression: "MIT OR BSD-3-Clause" }] }
      ]
    },
    draft: { artifactSubjectDigest: hex("a"), unit: { id: "regional-session-api" } },
    grype: { matches: [], descriptor: { db: { built: "2026-08-04", checksum: "sha256:db" } } },
    denyPolicy: { licenses: { allow: ["Apache-2.0", "MIT", "BSD-3-Clause"] } },
    denyPolicyBytes: "[licenses]\nallow=[]\n",
    syftVersion: "1.50.0",
    grypeVersion: "0.116.1",
    scannedAt: "2026-08-04T00:00:00Z"
  });

  expect((output.sbom.metadata as any).properties).toContainEqual({
    name: "aex:artifactSubjectDigest",
    value: hex("a")
  });
  expect(output.licenseInventory).toMatchObject({
    schema: "aex.license-inventory.v1",
    artifactSubjectDigest: hex("a")
  });
  expect(output.vulnerabilityVerdict).toMatchObject({
    scanner: "grype 0.116.1",
    unapprovedCritical: 0,
    unapprovedHigh: 0
  });
  expect(output.reports.sbom.checks).toHaveLength(2);
});

test("a Syft file SBOM uses metadata.component when no packages were detected", () => {
  const output = inspectSupplyChain({
    rawSbom: {
      bomFormat: "CycloneDX",
      specVersion: "1.6",
      metadata: {
        component: {
          "bom-ref": "84498d875ca5956e",
          type: "file",
          name: "artifact.bin",
          version: hex("d")
        }
      }
    },
    draft: { artifactSubjectDigest: hex("d"), unit: { id: "central-authz" } },
    grype: { matches: [], descriptor: { db: { built: "2026-08-05", checksum: "sha256:db" } } },
    denyPolicy: { licenses: { allow: ["Apache-2.0"] } },
    denyPolicyBytes: "[licenses]\nallow=[]\n",
    syftVersion: "1.50.0",
    grypeVersion: "0.116.1",
    scannedAt: "2026-08-05T00:00:00Z"
  });

  expect(output.licenseInventory.components).toEqual([{
    source: "metadata.component",
    name: "artifact.bin",
    version: hex("d"),
    licenseEvidence: "not-declared",
    licenses: [],
    denied: []
  }]);
});

test("unknown licenses and high vulnerabilities fail instead of becoming receipts", () => {
  const base = {
    draft: { artifactSubjectDigest: hex("b"), unit: { id: "hands-agent" } },
    denyPolicy: { licenses: { allow: ["Apache-2.0"] } },
    denyPolicyBytes: "policy",
    syftVersion: "1.50.0",
    grypeVersion: "0.116.1",
    scannedAt: "2026-08-04T00:00:00Z"
  } as const;
  expect(() => inspectSupplyChain({
    ...base,
    rawSbom: {
      bomFormat: "CycloneDX",
      specVersion: "1.6",
      components: [{ name: "unknown", licenses: [{ license: { id: "GPL-3.0-only" } }] }]
    },
    grype: { matches: [], descriptor: { db: { built: "today" } } }
  })).toThrow("license policy denied");
  expect(() => inspectSupplyChain({
    ...base,
    rawSbom: {
      bomFormat: "CycloneDX",
      specVersion: "1.6",
      components: [{ name: "known", licenses: [{ license: { id: "Apache-2.0" } }] }]
    },
    grype: {
      matches: [{ vulnerability: { severity: "High" } }],
      descriptor: { db: { built: "today" } }
    }
  })).toThrow("1 high vulnerabilities");
});

test("license exceptions apply only to the declared crate version", () => {
  const base = {
    draft: { artifactSubjectDigest: hex("c"), unit: { id: "central-control-api" } },
    denyPolicy: {
      licenses: {
        allow: ["Apache-2.0"],
        exceptions: [{ crate: "webpki-roots@1.0.9", allow: ["CDLA-Permissive-2.0"] }]
      }
    },
    denyPolicyBytes: "policy",
    syftVersion: "1.50.0",
    grypeVersion: "0.116.1",
    scannedAt: "2026-08-04T00:00:00Z",
    grype: { matches: [], descriptor: { db: { built: "today" } } }
  };
  const component = (version: string) => ({
    bomFormat: "CycloneDX",
    specVersion: "1.6",
    components: [
      { name: "webpki-roots", version, licenses: [{ license: { id: "CDLA-Permissive-2.0" } }] }
    ]
  });

  expect(() => inspectSupplyChain({ ...base, rawSbom: component("1.0.9") })).not.toThrow();
  expect(() => inspectSupplyChain({ ...base, rawSbom: component("1.0.10") })).toThrow(
    "license policy denied"
  );
});
