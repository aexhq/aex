import { defineWorkspace } from "vitest/config";

export default defineWorkspace([
  "packages/contracts",
  "packages/conformance",
  "packages/sdk",
  "packages/cli",
  "apps/user-tests",
  "scripts/validate"
]);
