import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const cargo = await readFile(path.join(root, "Cargo.toml"), "utf8");
const revision = cargo.match(
  /^brain-protocol\s*=\s*\{[^\n]*\brev\s*=\s*"([0-9a-f]{40})"/mu,
)?.[1];
assert.ok(revision, "brain-protocol must pin one immutable Brain revision");

const sdk = await readFile(path.join(root, "packages/sdk/src/index.ts"), "utf8");
assert.match(sdk, /from "@aexhq\/brain"/u);
assert.doesNotMatch(sdk, /interface Session\b/u);

const sessionContracts = await readFile(
  path.join(root, "../brain/crates/brain-http/generated/contract/session/v1/openapi.yaml"),
  "utf8",
);
assert.match(sessionContracts, /title: Brain HTTP API/u);

process.stdout.write(`verified Aex consumes Brain ${revision} without redefining session types\n`);
