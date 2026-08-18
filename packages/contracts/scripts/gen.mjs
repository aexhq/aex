// Generates src/abi.ts, src/session.ts (json-schema-to-typescript) and src/paths.ts
// (openapi-typescript) from contracts/. Run via tools/gen.sh; never hand-edit the outputs.
import { compile } from "json-schema-to-typescript";
import openapiTS, { astToString } from "openapi-typescript";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../../..");
const contracts = path.join(root, "contracts");
const out = path.join(here, "../src");
await mkdir(out, { recursive: true });

const banner = (src) =>
  `/* eslint-disable */\n/**\n * GENERATED from ${src} by packages/contracts/scripts/gen.mjs (tools/gen.sh). DO NOT EDIT.\n */\n`;

async function schemaToTs(rel, outName) {
  const schema = JSON.parse(await readFile(path.join(contracts, rel), "utf8"));
  const ts = await compile(schema, path.basename(rel), {
    bannerComment: banner(`contracts/${rel}`),
    additionalProperties: false,
    strictIndexSignatures: true,
    unreachableDefinitions: true,
    style: { singleQuote: false, printWidth: 100 },
    cwd: path.dirname(path.join(contracts, rel)),
  });
  await writeFile(path.join(out, outName), ts.replace(/\r\n/g, "\n"));
}
await schemaToTs("abi/v1/abi.json", "abi.ts");
await schemaToTs("session/v1/schemas.json", "session.ts");
await schemaToTs("control/v1/schemas.json", "control.ts");

const openapiPath = path.join(contracts, "session/v1/openapi.yaml");
const ast = await openapiTS(pathToFileURL(openapiPath), { exportType: true });
await writeFile(path.join(out, "paths.ts"), banner("contracts/session/v1/openapi.yaml") + astToString(ast));
const controlOpenapiPath = path.join(contracts, "control/v1/openapi.yaml");
const controlAst = await openapiTS(pathToFileURL(controlOpenapiPath), { exportType: true });
await writeFile(
  path.join(out, "control-paths.ts"),
  banner("contracts/control/v1/openapi.yaml") + astToString(controlAst),
);
// Runtime copies of the schemas and the sealed tool manifest, shipped with the package.
const schemasDir = path.join(here, "../schemas");
await mkdir(schemasDir, { recursive: true });
for (const [rel, name] of [
  ["abi/v1/abi.json", "abi.v1.json"],
  ["session/v1/schemas.json", "session.v1.json"],
  ["control/v1/schemas.json", "control.v1.json"],
  ["abi/v1/tools/manifest.json", "tools.manifest.v1.json"],
  ["abi/v1/tools/manifest.digest", "tools.manifest.v1.digest"],
]) {
  await writeFile(path.join(schemasDir, name), await readFile(path.join(contracts, rel)));
}
console.log("generated abi.ts session.ts control.ts paths.ts control-paths.ts schemas/");
