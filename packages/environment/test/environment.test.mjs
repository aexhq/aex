import assert from "node:assert/strict";
import test from "node:test";

import {
  computer,
  defineEnvironment,
  inspectEnvironment,
  isEnvironmentRef,
  linux,
} from "../dist/index.js";

test("an environment factory returns an opaque immutable typed reference", () => {
  const example = defineEnvironment({
    identity: "example.environment",
    protocol: "environment/v1",
    profile: computer({ platform: linux.amd64, network: "allowlist", recovery: "retained" }),
    serialize: ({ region }) => ({ region }),
    handle: (context, { region }) => ({ region, sessionId: context.sessionId }),
  });
  const ref = example({ region: "eu-west-2" });
  const descriptor = inspectEnvironment(ref);

  assert.equal(isEnvironmentRef(ref), true);
  assert.equal(Object.isFrozen(ref), true);
  assert.deepEqual(descriptor.serialized, {
    extension: "example.environment",
    protocol: "environment/v1",
    profile: {
      kind: "computer",
      platform: "linux-amd64",
      network: "allowlist",
      recovery: "retained",
    },
    configuration: { region: "eu-west-2" },
  });
});

test("plain objects cannot impersonate environment references", () => {
  assert.equal(isEnvironmentRef({}), false);
  assert.throws(() => inspectEnvironment({}), /not an EnvironmentRef/);
});
