---
title: generated TypeScript wire binding
description: Why the public TypeScript wire artifact is generated from ContractIr, what it owns, and how consumers migrate from the deleted handwritten contracts package.
keywords:
  - TypeScript
  - wire contract
  - generation
  - Zod
  - migration
audience: implementation agents and maintainers
status: accepted
related:
  - references/architecture.md
  - references/repo.md
  - references/rules.md
  - references/rewrite/contracts.md
---

# Generated TypeScript wire binding

## Problem

The Rust-native cut deleted the handwritten `@aexhq/contracts` package. Its
types and validators predated the authored strict-v1 schema tree and could
drift independently from `aex-wire`. TypeScript consumers still need runtime
validation, but restoring that source would restore a second contract
authority.

The SDK is not that authority. It owns client ergonomics and transport. Making
hosted code import SDK internals would couple wire validation to client policy
and would still leave non-SDK consumers without canonical schemas.

## Decision

`@aexhq/wire` is a generated public package. `aex-contract-gen` emits its
models and strict Zod validators from the same in-memory `ContractIr` that
emits `aex-wire`, JSON Schema, OpenAPI, and the conformance corpus. The package
adds no authored model declarations and exposes one root entrypoint.

The binding includes:

- one TypeScript type and one closed runtime validator for every authored
  schema;
- the contract digest carried by every other generated binding;
- the generated identifier registry and UUIDv7-aware parse, assert, and mint
  helpers;
- explicit recursive types and lazy validator back-edges only for recursive
  unions. All acyclic schema references remain eager, so a missing dependency
  fails module initialization rather than appearing during a request.

Scalar validators preserve the Rust wire invariants that plain JSON Schema
keywords cannot fully express, including UTF-8 byte bounds, UUIDv7 version and
variant bits, real millisecond timestamps, nonzero W3C trace/span identifiers,
normalized absolute POSIX paths, cursor grammar, and non-empty byte ranges.

## Consumer migration

Consumers migrate to canonical names and shapes from `@aexhq/wire`. The new
package does not export compatibility aliases for the deleted handwritten
surface. Where an old declaration has no current schema equivalent, the
consumer must be changed or removed; adding an approximation to the generated
package would make the compiler green by making the contract false.

The migration can proceed package by package, but a consumer repository must
not claim a green cross-repository contract gate until every compiled or
deployed target has left `@aexhq/contracts`. An exact immutable snapshot may
cache built `@aexhq/wire` bytes, but it must preserve package manifest and
content identity and must never synthesize extra exports.

## Trade-offs

- **Correctness:** one IR emits both language bindings, removing semantic
  mirror drift. Clean-cut renames make stale models visible as compile errors.
- **Reliability:** generated output is checked into the repository and covered
  by deterministic drift tests, so builds do not depend on a registry or an
  unpinned code generator.
- **Performance:** validators are static Zod programs. Acyclic references are
  direct constants; only true recursive edges pay lazy dispatch.
- **Scalability:** adding a schema grows one generator path rather than a Rust
  type plus a separately maintained TypeScript declaration. The package is an
  independent artifact, so services that do not use TypeScript do not load it.
- **Migration cost:** downstream consumers must adopt changed canonical shapes
  rather than retaining aliases. That cost is deliberate in prelaunch because
  paying it once prevents permanent split authority.
