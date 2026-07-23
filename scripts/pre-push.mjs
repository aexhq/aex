#!/usr/bin/env node

import { spawnSync } from "node:child_process";

const bun = process.versions.bun
  ? process.execPath
  : process.platform === "win32"
    ? "bun.exe"
    : "bun";

for (const check of ["lint", "typecheck"]) {
  console.log(`aex pre-push: bun run ${check}`);
  const result = spawnSync(bun, ["run", check], { stdio: "inherit" });
  if (result.error) {
    console.error(`aex pre-push: failed to start ${check}: ${result.error.message}`);
    process.exit(1);
  }
  if (result.signal) {
    console.error(`aex pre-push: ${check} terminated by ${result.signal}`);
    process.exit(1);
  }
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

console.log("aex pre-push: lint and typecheck passed");
