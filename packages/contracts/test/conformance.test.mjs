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

test("exact integer strings are canonical and numeric fields are bounded", () => {
  const ajv = new Ajv2020({ strict: false, allErrors: true });
  const schema = JSON.parse(CONTROL_SCHEMA_JSON);
  delete schema.$id;
  ajv.addSchema(schema, "root");
  const signed = ajv.getSchema("root#/$defs/MicroUsd");
  const unsigned = ajv.getSchema("root#/$defs/UnsignedDecimalInteger");
  assert.ok(signed);
  assert.ok(unsigned);

  for (const value of ["0", "1", "-1", "9223372036854775807", "-9223372036854775808"]) {
    assert.equal(signed(value), true, `rejected signed ${value}`);
  }
  for (const value of ["", "-0", "+1", "00", "01", "-01", "1.0", "1e3", " 1", 1]) {
    assert.equal(signed(value), false, `accepted non-canonical signed ${JSON.stringify(value)}`);
  }
  for (const value of ["0", "1", "9007199254740992", "9223372036854775807"]) {
    assert.equal(unsigned(value), true, `rejected unsigned ${value}`);
  }
  for (const value of ["", "-0", "-1", "+1", "00", "01", "1.0", "1e3", " 1", 0]) {
    assert.equal(unsigned(value), false, `accepted non-canonical unsigned ${JSON.stringify(value)}`);
  }

  const limits = ajv.getSchema("root#/$defs/AccountLimits");
  assert.equal(limits({ max_concurrent_sessions: 1, session_creates_per_hour: 1_000_000 }), true);
  assert.equal(limits({ max_concurrent_sessions: 0, session_creates_per_hour: 1 }), false);
  assert.equal(limits({ max_concurrent_sessions: 1, session_creates_per_hour: 1_000_001 }), false);

  const topup = ajv.getSchema("root#/$defs/CreateTopupRequest");
  assert.equal(topup({ amount_cents: 1_000 }), true);
  assert.equal(topup({ amount_cents: 100_000 }), true);
  assert.equal(topup({ amount_cents: 999 }), false);
  assert.equal(topup({ amount_cents: 100_001 }), false);

  const storage = ajv.getSchema("root#/$defs/StorageMeters");
  assert.equal(storage({ session_storage_bytes: 10_737_418_240, upload_reserved_bytes: 0 }), true);
  assert.equal(storage({ session_storage_bytes: 10_737_418_241, upload_reserved_bytes: 0 }), false);
});
