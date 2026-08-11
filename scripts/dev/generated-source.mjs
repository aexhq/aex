#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const mode = process.argv[2];

if (mode !== "build" && mode !== "check") {
  console.error("usage: bun run generate[:check]");
  process.exit(2);
}

const commands =
  mode === "build"
    ? [
        ["contract and SDK bindings", ["run", "--locked", "-p", "aex-contract-gen", "--", "build"]],
        [
          "regional table bundle",
          ["run", "--locked", "-p", "aex-regional-test-support", "--example", "emit-regional-tables"],
        ],
        ["test and evidence registries", ["run", "--locked", "-p", "aex-workspace-check", "--", "registry", "build"]],
      ]
    : [
        ["contract and SDK bindings", ["run", "--locked", "-p", "aex-contract-gen", "--", "check"]],
        [
          "regional table bundle",
          [
            "test",
            "--locked",
            "-p",
            "aex-regional-test-support",
            "tables::tests::the_generated_bundle_matches_a_deterministic_rebuild",
            "--",
            "--exact",
          ],
        ],
        ["test and evidence registries", ["run", "--locked", "-p", "aex-workspace-check", "--", "registry", "verify"]],
      ];

for (const [label, args] of commands) {
  console.log(`aex generated source: ${mode} ${label}`);
  const result = spawnSync("cargo", args, { cwd: root, stdio: "inherit" });
  if (result.error) {
    console.error(`aex generated source: failed to start cargo: ${result.error.message}`);
    process.exit(1);
  }
  if (result.signal) {
    console.error(`aex generated source: ${label} terminated by ${result.signal}`);
    process.exit(1);
  }
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

console.log(`aex generated source: ${mode} complete`);
