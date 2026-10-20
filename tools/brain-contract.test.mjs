import assert from 'node:assert/strict';
import test from 'node:test';
import { generate } from './brain-contract.mjs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { brainSource } from './runtime-source.mjs';

test('Brain DTOs are generated from the pinned contract including optional private admission and executor fields', async () => {
  const contract = JSON.parse(await readFile(new URL('../contracts/brain.openapi.json', import.meta.url)));
  assert.equal(await readFile(join(brainSource().Dir,'api/openapi.json'),'utf8'),await readFile(new URL('../contracts/brain.openapi.json',import.meta.url),'utf8'),'the DTO source must match the pinned runtime API');
  const source = generate(contract);
  assert.equal(source, await readFile(new URL('../crates/aex-server/src/brain_contract.rs', import.meta.url), 'utf8'));
  assert.match(source, /pub model_credential: Option<ModelCredential>/);
  assert.match(source, /pub input_id: Option<String>/);
  assert.match(source, /pub r#type: String/);
  assert.match(source, /pub arguments: serde_json::Value/);
  assert.match(source, /pub struct ExecutorRegistration/);
  assert.match(source, /deny_unknown_fields/);
  assert.throws(() => generate({components:{schemas:{Invalid:{type:'object',properties:{broken:{type:'future'}}}}}}), /unsupported/);
});
