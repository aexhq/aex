---
title: ARM64 release and qualification plan
description: Test-first plan for ARM64 artifact execution receipts, readiness, load qualification, and rollback.
keywords:
  - arm64
  - release
  - qualification
  - ci
audience: implementation agents and maintainers
status: proposed
related:
  - references/rewrite/README.md
  - references/rewrite/architecture-performance-v1.md
---

# ARM64 release and qualification plan

ARM64 is the only v1 runtime architecture. x86 artifacts, fallback images, and
mixed-architecture production services are out of scope for the clean cut.

## Current gap

The repository cross-compiles AArch64 artifacts and declares ARM64 in active
Terraform, but current CI mostly validates ELF metadata and image manifests on
x86 runners. It does not require execution-based architecture evidence.

## Required implementation

### Artifact receipt

Add an artifact-bound `arch-qualification` receipt containing:

- source SHA;
- exact artifact/image digest;
- target `aarch64` identity;
- host/executor identity;
- bootstrap/startup result;
- dependency/loader result;
- timestamp and expiry;
- workload smoke results.

Release admission must require this receipt for every ARM Lambda, OCI service,
one-shot task, Hands agent, and Hands image.

### CI execution

The public workflow must execute the exact ARM bytes, not only cross-compile:

- ARM Lambda bootstrap smoke;
- ARM OCI container startup/health smoke;
- ARM64 Hands-agent and image checks;
- representative Brain/session/stream request startup;
- AWS-LC, compression, serialization, regex, and filesystem checks.

Execution may use native ARM CI capacity or a faithful emulator, but the dev
plane must provide final real-host evidence before production promotion.

### Readiness

Post-apply readiness must verify:

- Lambda `Architectures` contains only `arm64`;
- ECS task definition `runtimePlatform.cpuArchitecture` is `ARM64`;
- exact image/code digests match the release manifest;
- ARM health/readiness probes succeed;
- one-task launch remains stable before scale tests.

### Performance receipts

Run on ARM in dev:

- 100 active Brain target;
- 200 active safety;
- 500 offered overload;
- 1 MiB context restore;
- long streams;
- provider/web/MCP/Hands mixes;
- RSS, file descriptors, queue age, stalls, lost work;
- cost per completed turn.

The Brain task size may be reduced only when the receipt proves the configured
concurrency and context budget remain below RSS and queue limits.

### Rollback

Rollback is to the previous immutable ARM release. There is no x86 sibling
rollback path. Lambda alias, ECS task definition, and MicroVM generation pins
must all be retained as exact rollback identities.
