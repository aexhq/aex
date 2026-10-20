import { readFile, writeFile } from "node:fs/promises";
import { compile } from "json-schema-to-typescript";
const schemas = ["client-access", "client-grant-input", "composition", "tool-catalog", "upload-grant-input", "upload-grant", "attachment-limits", "attachment", "account", "usage", "api-key", "issued-key", "key-input", "login-grant-input", "login-grant", "login-exchange", "account-session", "wallet", "billing-settings", "ledger-page", "topup-input", "topup", "refund-input", "refund", "sync-payment"];
const root = new URL("../", import.meta.url);
const definitions = {};
for (const name of schemas) {
  const schema = JSON.parse(await readFile(new URL(`docs/generated/${name}.schema.json`, root), "utf8"));
  Object.assign(definitions, schema.$defs);
  delete schema.$defs;
  delete schema.$schema;
  definitions[schema.title] = schema;
}
// Shared wire contracts remain owned by the pinned Brain SDK.
const shared = Object.keys(JSON.parse(await readFile(new URL("contracts/brain.openapi.json", root),"utf8")).components.schemas);
for (const name of shared) delete definitions[name];
let serialized=JSON.stringify({type:"object",properties:{},$defs:definitions});
for (const name of shared) serialized=serialized.replaceAll(`{"$ref":"#/$defs/${name}"}`,JSON.stringify({tsType:name}));
const schema=JSON.parse(serialized.replaceAll("#/$defs/", "#/definitions/").replace('"$defs":','"definitions":'));
const output=await compile(schema,"AexContracts",{unreachableDefinitions:true,bannerComment:`/* Generated from aex-server schemas. */\nimport type { ${shared.join(", ")} } from "@aexhq/brain";`,additionalProperties:false});
await writeFile(new URL("packages/sdk/src/generated.ts", root), output);
