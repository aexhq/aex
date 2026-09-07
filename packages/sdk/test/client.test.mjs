import assert from "node:assert/strict";
import test from "node:test";
import { Aex, Brain, tool, hostEnv, brainEnv } from "../dist/index.js";
import * as upstream from "@aexhq/brain";

test("Aex preserves Brain and its extension identities and uses the account API", async () => {
  assert.equal(Brain, upstream.Brain);
  assert.equal(tool, upstream.tool);
  assert.equal(hostEnv, upstream.hostEnv);
  assert.equal(brainEnv, upstream.brainEnv);
  const requests=[];
  const client = new Aex({apiKey:"customer-key", fetch:async(url,init)=>{
    requests.push({url,init});
    return Response.json({id:"account"});
  }});
  assert.ok(client instanceof upstream.Brain);
  assert.equal((await client.account.get()).id,"account");
  assert.equal(requests[0].url,"https://api.aex.dev/v1/account");
  assert.equal(new Headers(requests[0].init.headers).get("authorization"),"Bearer customer-key");
});
