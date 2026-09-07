import { readFile, writeFile } from "node:fs/promises";
import { compile } from "json-schema-to-typescript";
const schemas = ["account", "usage", "api-key", "issued-key", "key-input", "login-grant-input", "login-grant", "login-exchange", "account-session"];
const root = new URL("../", import.meta.url);
const definitions = {};
for (const name of schemas) {
  const schema = JSON.parse(await readFile(new URL(`docs/generated/${name}.schema.json`, root), "utf8"));
  Object.assign(definitions, schema.$defs);
  delete schema.$defs;
  delete schema.$schema;
  definitions[schema.title] = schema;
}
const schema = JSON.parse(JSON.stringify({type:"object",properties:{},$defs:definitions}).replaceAll("#/$defs/", "#/definitions/").replace('"$defs":', '"definitions":'));
const output = await compile(schema, "AexContracts", { unreachableDefinitions:true, bannerComment:"/* Generated from aex-server schemas. */", additionalProperties:false });
await writeFile(new URL("packages/sdk/src/generated.ts", root), output);
