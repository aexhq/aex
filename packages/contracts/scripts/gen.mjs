// Generates Aex-owned control-plane types from contracts/control/v1.
import { compile } from "json-schema-to-typescript";
import openapiTS, { astToString } from "openapi-typescript";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../../..");
const contracts = path.join(root, "contracts");
const out = path.join(here, "../src");
await mkdir(out, { recursive: true });

const banner = (src) =>
  `/* eslint-disable */\n/**\n * GENERATED from ${src} by packages/contracts/scripts/gen.mjs (tools/gen.sh). DO NOT EDIT.\n */\n`;

const schemaRel = "control/v1/schemas.json";
const schema = JSON.parse(await readFile(path.join(contracts, schemaRel), "utf8"));
const types = await compile(schema, path.basename(schemaRel), {
  bannerComment: banner(`contracts/${schemaRel}`),
  additionalProperties: false,
  strictIndexSignatures: true,
  unreachableDefinitions: true,
  style: { singleQuote: false, printWidth: 100 },
  cwd: path.dirname(path.join(contracts, schemaRel)),
});
await writeFile(path.join(out, "control.ts"), types.replace(/\r\n/g, "\n"));

const openapiRel = "control/v1/openapi.yaml";
const ast = await openapiTS(pathToFileURL(path.join(contracts, openapiRel)), { exportType: true });
await writeFile(
  path.join(out, "control-paths.ts"),
  banner(`contracts/${openapiRel}`) + astToString(ast),
);

const schemasDir = path.join(here, "../schemas");
await mkdir(schemasDir, { recursive: true });
await writeFile(
  path.join(schemasDir, "control.v1.json"),
  await readFile(path.join(contracts, schemaRel)),
);
console.log("generated control.ts control-paths.ts schemas/control.v1.json");
