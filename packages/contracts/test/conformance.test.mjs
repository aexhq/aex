// Mirrors crates/aex-contracts/tests/conformance.rs: every example validates against the schema
// type named by its filename, the manifest digest matches the pin, and callHash agrees with the
// call_hash values carried by the start examples (computed independently in Python and Rust).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

import * as contracts from "../dist/index.js";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const examplesDir = (d) => path.join(root, "contracts/examples", d);

function validatorFor(schemaJson) {
  const ajv = new Ajv2020({ strict: false, allErrors: true });
  addFormats(ajv);
  const schema = JSON.parse(schemaJson);
  delete schema.$id;
  ajv.addSchema(schema, "root");
  return (typeName, value) => {
    const validate = ajv.getSchema(`root#/$defs/${typeName}`);
    assert.ok(validate, `no $defs/${typeName} in schema`);
    const ok = validate(value);
    return ok ? [] : validate.errors.map((e) => `${e.instancePath} ${e.message}`);
  };
}

function examples(dir) {
  return readdirSync(examplesDir(dir))
    .filter((f) => f.endsWith(".json"))
    .map((f) => ({ name: f, typeName: f.split(".")[0], value: JSON.parse(readFileSync(path.join(examplesDir(dir), f), "utf8")) }));
}

test("abi examples validate against the schema", () => {
  const validate = validatorFor(contracts.ABI_SCHEMA_JSON);
  const ex = examples("abi");
  assert.ok(ex.length > 0);
  for (const { name, typeName, value } of ex) {
    assert.deepEqual(validate(typeName, value), [], name);
  }
});

test("session examples validate against the schema", () => {
  const validate = validatorFor(contracts.SESSION_SCHEMA_JSON);
  const ex = examples("session");
  assert.ok(ex.length > 0);
  for (const { name, typeName, value } of ex) {
    assert.deepEqual(validate(typeName, value), [], name);
  }
});

test("control examples validate against the schema", () => {
  const validate = validatorFor(contracts.CONTROL_SCHEMA_JSON);
  const ex = examples("control");
  assert.ok(ex.length > 0);
  for (const { name, typeName, value } of ex) {
    assert.deepEqual(validate(typeName, value), [], name);
  }
});

test("tool manifest digest matches the pin", () => {
  const manifest = contracts.toolManifestV1();
  const validate = validatorFor(contracts.ABI_SCHEMA_JSON);
  assert.deepEqual(validate("ToolManifest", manifest), []);
  assert.equal(contracts.manifestDigest(manifest), contracts.TOOL_MANIFEST_V1_DIGEST);
  assert.match(contracts.TOOL_MANIFEST_V1_DIGEST, /^[0-9a-f]{64}$/);
});

test("callHash agrees with every start example", () => {
  let seen = 0;
  for (const { name, typeName, value } of examples("abi")) {
    if (typeName !== "Request" || value.call.op !== "start") continue;
    assert.equal(contracts.callHash(value.call.args), value.call.args.call_hash, name);
    seen += 1;
  }
  assert.ok(seen >= 2);
});
