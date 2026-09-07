import assert from "node:assert/strict";
import { Brain, agentloop, brainEnv, component } from "@aexhq/brain";
import { pathToFileURL } from "node:url";

for(const key of ["AEX_URL","AEX_API_KEY","AEX_MODEL_KEY","BRAIN_TEST_REFERENCE_AGENTLOOP"]) assert.ok(process.env[key],`${key} required`);
const brain=new Brain({baseUrl:process.env.AEX_URL,token:process.env.AEX_API_KEY});
let session;
try {
  session=await brain.sessions.create({model:{provider:"vercel-ai-gateway",name:"openai/gpt-4.1-mini",apiKey:process.env.AEX_MODEL_KEY},agentloop:agentloop({implementation:component(pathToFileURL(process.env.BRAIN_TEST_REFERENCE_AGENTLOOP))})({env:brainEnv({name:"brain"})})});
  await session.send("Reply with exactly: hosted session works");
  const events=[];for await(const event of session.events())events.push(event);
  assert.ok(events.some(event=>event.type==="model_call_ended"),"real model call must commit a success event");
  assert.ok(!events.some(event=>event.type==="model_call_failed"),"real model call failed");
  console.log("real provider hosted session passed");
} finally {
  if(session){await session.end();await session.delete();}
}
