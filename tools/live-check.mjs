import assert from "node:assert/strict";
import { Aex, brainEnv, hostEnv, tool } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { randomUUID } from "node:crypto";
import { z } from "zod";

for(const key of ["AEX_URL","AEX_API_KEY","AEX_MODEL_KEY"]) assert.ok(process.env[key],`${key} required`);
const brain=new Aex({baseUrl:process.env.AEX_URL,apiKey:process.env.AEX_API_KEY});
let session, registration, toolCalls=0;
const answer=randomUUID();
const probe=tool({name:"hosted_probe",description:"Return the current hosting verification value",input:z.object({}),run:()=>{toolCalls++;return answer;}});
try {
  session=await brain.sessions.create({model:{provider:"vercel-ai-gateway",name:"openai/gpt-4.1-mini",apiKey:process.env.AEX_MODEL_KEY},agentloop:pi({env:brainEnv({name:"brain"})}),tools:[probe({env:hostEnv({name:"application"})})]});
  registration=await brain.register();
  await session.send("Call hosted_probe exactly once, then reply with its returned value. Do not invent the value.");
  assert.equal(toolCalls,1,"the real provider must invoke the application Tool");
  const events=[];for await(const event of session.events())events.push(event);
  assert.ok(events.some(event=>event.type==="model_call_ended"),"real model call must commit a success event");
  assert.ok(!events.some(event=>event.type==="model_call_failed"),"real model call failed");
  assert.ok(events.some(event=>event.type==="tool_call_ended"),"application Tool result must commit");
  console.log("real provider and application Tool hosted session passed");
} finally {
  try {if(session){await session.end();await session.delete();}}
  finally {if(registration){registration.pump.stop();await registration.pump.closed;}}
}
