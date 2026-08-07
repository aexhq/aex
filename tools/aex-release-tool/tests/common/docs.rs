//! Valid release documents, built once and mutated per test.
//!
//! Each builder returns a document that both authorities accept. A test then
//! changes exactly the one thing it is about, which is what makes a failure
//! message point at a cause rather than at a diff.

#![allow(dead_code)]

use serde_json::{Value, json};

/// A well-formed `sha256:` digest whose bytes are all `byte`.
#[must_use]
pub fn digest(byte: u8) -> String {
    format!("sha256:{}", format!("{byte:02x}").repeat(32))
}

/// A well-formed 40-hex commit id.
#[must_use]
pub fn sha1() -> String {
    "a".repeat(40)
}

/// The builder identity every fixture claims.
pub const BUILDER: &str = "https://github.com/aexhq/aex/.github/workflows/main.yml@refs/heads/main";

/// A valid `aex.artifact-envelope.v1`.
#[must_use]
pub fn valid_envelope() -> Value {
    json!({
        "schema": "aex.artifact-envelope.v1",
        "envelopeDigest": digest(1),
        "artifactSubjectDigest": digest(2),
        "unit": { "id": "regional-session-api", "kind": "rust-lambda", "plane": "regional" },
        "media": { "mediaType": "application/zip", "form": "zip" },
        "source": {
            "repository": "aexhq/aex",
            "commitSha": sha1(),
            "treeClean": true,
            "ref": "refs/heads/main",
            "workflow": {
                "repository": "aexhq/aex",
                "ref": "refs/heads/main",
                "path": ".github/workflows/_build-artifacts.yml",
                "runId": "123",
                "runAttempt": 1,
                "jobName": "build",
                "builderId": BUILDER
            }
        },
        "inputs": {
            "lockfileDigest": digest(2),
            "toolchain": {
                "channel": "1.97.1",
                "rustcVersion": "1.97.1",
                "rustcCommitHash": sha1(),
                "host": "x86_64-unknown-linux-gnu",
                "target": "aarch64-unknown-linux-gnu.2.34"
            },
            "buildProfile": "release-lambda",
            "buildCommand": { "argv": ["cargo", "lambda", "build"], "env": {}, "digest": digest(3) },
            "inputClosureDigest": digest(4)
        },
        "output": {
            "digest": digest(5),
            "sizeBytes": 4096,
            "target": {
                "os": "linux",
                "architecture": "arm64",
                "triple": "aarch64-unknown-linux-gnu.2.34",
                "libcVersion": "2.34"
            },
            "location": {
                "kind": "s3",
                "uri": "lambda/regional-session-api/deadbeef.zip",
                "immutable": true
            }
        },
        "identities": { "contractDigest": digest(6), "configSchemaVersion": 1 },
        "composition": {
            "minimum": [],
            "adjacent": {
                "storageCompatible": true,
                "protocolCompatible": true,
                "rollbackEligible": true,
                "rationale": "no durable state shape changed in this version"
            }
        },
        "sbom": {
            "format": "cyclonedx-1.6",
            "digest": digest(7),
            "uri": "sbom/regional-session-api.json",
            "componentCount": 214
        },
        "licenses": { "policyDigest": digest(8), "verdict": "allowed", "denials": [] },
        "vulnerabilities": {
            "scanner": "cargo-audit",
            "database": "rustsec-2026-08-01",
            "scannedAt": "2026-08-01T00:00:00Z",
            "unapprovedCritical": 0,
            "unapprovedHigh": 0
        },
        "provenance": {
            "predicateType": "https://slsa.dev/provenance/v1",
            "bundleDigest": digest(9),
            "builderId": BUILDER,
            "attested": true
        },
        "signature": { "present": false, "kind": "none" },
        "receipts": [{
            "class": "unit",
            "receiptDigest": digest(10),
            "source": {
                "repository": "aexhq/aex",
                "commitSha": sha1(),
                "workflowRunId": "123",
                "runAttempt": 1
            },
            "conclusion": "passed"
        }],
        "retention": { "class": "artifact-bytes" },
        "createdAt": "2026-08-01T00:00:00Z"
    })
}

/// A valid `aex.composition-manifest.v1` holding the one unit above.
#[must_use]
pub fn valid_manifest() -> Value {
    json!({
        "schema": "aex.composition-manifest.v1",
        "releaseId": digest(0x11),
        "contractDigest": digest(6),
        "source": {
            "repository": "aexhq/aex",
            "commitSha": sha1(),
            "workflowRunId": "123",
            "workflowRunAttempt": 1
        },
        "releaseTool": {
            "version": "0.1.0",
            "digest": digest(0x27),
            "sizeBytes": 8192,
            "uri": format!("https://github.com/aexhq/aex/releases/download/main-{}-run-123-attempt-1/aex-release-tool", sha1()),
            "target": "x86_64-unknown-linux-musl"
        },
        "units": {
            "regional-session-api": {
                "kind": "rust-lambda",
                "envelopeDigest": digest(1),
                "artifactDigest": digest(5),
                "sizeBytes": 4096,
                "location": {
                    "kind": "s3",
                    "uri": "lambda/regional-session-api/deadbeef.zip",
                    "immutable": true
                },
                "target": {
                    "os": "linux",
                    "architecture": "arm64",
                    "triple": "aarch64-unknown-linux-gnu.2.34"
                },
                "configSchemaVersion": 1,
                "lambda": {
                    "memoryMiB": 1024,
                    "timeoutS": 30,
                    "reservedConcurrency": 8
                },
                "adjacent": {
                    "storageCompatible": true,
                    "protocolCompatible": true,
                    "rollbackEligible": true,
                    "rationale": "no durable state shape changed"
                }
            }
        },
        "migrations": {
            "central": {
                "bundleDigest": digest(0x21),
                "head": "20260801000100",
                "adminImageDigest": digest(0x22)
            },
            "regional": {
                "bundleDigest": digest(0x23),
                "bundleSizeBytes": 71384,
                "bundleUri": format!("https://github.com/aexhq/aex/releases/download/main-{}-run-123-attempt-1/regional-tables.json", sha1()),
                "definitionsDigest": format!("blake3:{}", "23".repeat(32)),
                "generation": 1
            }
        },
        "infra": {
            "moduleBundleDigest": digest(0x24),
            "moduleBundleSizeBytes": 16384,
            "moduleBundleUri": format!("https://github.com/aexhq/aex/releases/download/main-{}-run-123-attempt-1/terraform-modules.tar.gz", sha1()),
            "terraformVersion": "1.14.0",
            "providerVersions": { "hashicorp/aws": "6.0.0" }
        },
        "order": [{
            "name": "regional-api",
            "units": ["regional-session-api"],
            "mode": "parallel",
            "rationale": "default order stage; nothing in it depends on anything else in it"
        }],
        "policy": {
            "toolchainChannel": "1.97.1",
            "artifactPolicyDigest": digest(0x25),
            "freshnessPolicyDigest": digest(0x26),
            "sourcePolicyVersion": 1
        }
    })
}

/// A valid `aex.evidence-receipt.v1`.
#[must_use]
pub fn valid_receipt() -> Value {
    json!({
        "schema": "aex.evidence-receipt.v1",
        "receiptDigest": digest(0x31),
        "receiptId": "rc_01",
        "class": "unit",
        "layer": "unit",
        "lane": "pr",
        "concerns": ["property"],
        "source": {
            "repository": "aexhq/aex",
            "commitSha": sha1(),
            "treeClean": true,
            "workflowRunId": "123",
            "runAttempt": 1,
            "jobName": "rust",
            "builderId": "github-hosted"
        },
        "selection": { "mode": "affected", "filterset": "all()", "packages": ["aex-wire"] },
        "inventory": {
            "declared": 10, "collected": 10, "passed": 10, "failed": 0,
            "skipped": 0, "ignored": 0, "filteredAtRuntime": 0, "retried": 0, "flaky": 0
        },
        "data": { "residue": "none", "secretCanaryObserved": false },
        "startedAt": "2026-08-01T00:00:00Z",
        "completedAt": "2026-08-01T00:05:00Z",
        "conclusion": "passed"
    })
}

/// A valid `aex.verification-statement.v1` for the manifest above.
#[must_use]
pub fn valid_statement() -> Value {
    json!({
        "schema": "aex.verification-statement.v1",
        "statementDigest": digest(0x41),
        "releaseId": digest(0x11),
        "plane": "dev",
        "bindingDigest": digest(0x42),
        "bindingRef": sha1(),
        "regions": ["eu-west-1"],
        "deployed": [{
            "unit": "regional-session-api",
            "expectedDigest": digest(5),
            "actualDigest": digest(5),
            "actualVersionOrAlias": "live",
            "readbackAt": "2026-08-01T01:00:00Z"
        }],
        "receipts": [{
            "class": "smoke",
            "receiptDigest": digest(0x31),
            "conclusion": "passed",
            "collectedAt": "2026-08-01T01:00:00Z"
        }],
        "startedAt": "2026-08-01T00:30:00Z",
        "completedAt": "2026-08-01T01:00:00Z",
        "conclusion": "passed",
        "attestation": {
            "predicateType": "https://slsa.dev/provenance/v1",
            "bundleDigest": digest(0x43),
            "builderId": "github-hosted"
        },
        "ledgerFence": 7
    })
}
