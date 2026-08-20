import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

import { CONTROL_SCHEMA_JSON } from "../dist/index.js";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const examplesDir = path.join(root, "contracts/examples/control");

test("Aex control examples validate against the Aex-owned schema", () => {
  const ajv = new Ajv2020({ strict: false, allErrors: true });
  addFormats(ajv);
  const schema = JSON.parse(CONTROL_SCHEMA_JSON);
  delete schema.$id;
  ajv.addSchema(schema, "root");

  const files = readdirSync(examplesDir).filter((file) => file.endsWith(".json"));
  assert.ok(files.length > 0);
  for (const file of files) {
    const typeName = file.split(".")[0];
    const validate = ajv.getSchema(`root#/$defs/${typeName}`);
    assert.ok(validate, `${file}: no $defs/${typeName} in schema`);
    const value = JSON.parse(readFileSync(path.join(examplesDir, file), "utf8"));
    assert.equal(validate(value), true, `${file}: ${JSON.stringify(validate.errors)}`);
  }
});
