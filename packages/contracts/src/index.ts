/** Aex-owned account, billing, and control-plane contracts. */
import { readFileSync } from "node:fs";

export * as control from "./control.js";
export type {
  paths as controlPaths,
  components as controlComponents,
  operations as controlOperations,
} from "./control-paths.js";

/** Raw Aex control-plane JSON Schema (parse on demand). */
export const CONTROL_SCHEMA_JSON: string = readFileSync(
  new URL("../schemas/control.v1.json", import.meta.url),
  "utf8",
);
