import assert from "node:assert/strict";
import test from "node:test";
import { authenticateApplication } from "../environments/application.mjs";

test("sealed application credentials authenticate the private bridge and expose only request-local authority", () => {
  const token = "bridge-secret".repeat(3);
  const authorization = `http_${"a".repeat(64)}`;
  const credential = "customer-secret".repeat(3);
  const bearer = value => ({ authorization: `Bearer application.${Buffer.from(JSON.stringify(value)).toString("base64url")}` });
  const envelope = { version: 1, token, authorization, credential };
  assert.deepEqual(authenticateApplication(bearer(envelope), token), { authorization, credential });
  assert.equal(authenticateApplication({ authorization: `Bearer ${token}` }, token), undefined);
  for (const changed of [{ token: "forged" }, { version: 2 }, { authorization: "other" }, { credential: "" }]) {
    assert.throws(() => authenticateApplication(bearer({ ...envelope, ...changed }), token));
  }
  for (const headers of [{}, { authorization: "Bearer invalid" }, { authorization: "Bearer application.invalid" }]) {
    assert.throws(() => authenticateApplication(headers, token));
  }
});
