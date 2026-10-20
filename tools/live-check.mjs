import assert from 'node:assert/strict';
import { Aex,ApiKey,Model,Tool } from '@aexhq/sdk';
import { randomUUID } from 'node:crypto';
import { z } from 'zod';

for(const key of ['AEX_URL','AEX_API_KEY','AEX_MODEL_KEY'])assert.ok(process.env[key],`${key} required`);
const client=new Aex({baseUrl:process.env.AEX_URL,apiKey:process.env.AEX_API_KEY,maxCostMicroUsd:1000000});
try {
  for(const harness of ['pi','codex','opencode']) {
    let calls=0;const answer=randomUUID();
    const session=await client.sessions.create({harness,model:Model(process.env.AEX_MODEL??'vercel-ai-gateway/openai/gpt-4.1-mini',ApiKey(process.env.AEX_MODEL_KEY)),tools:[Tool({name:'probe',description:'Return the verification value',input_schema:{type:'object',properties:{},additionalProperties:false},run:()=>{calls++;return answer;}})]});
    try {
      const turn=await session.send('Call runtime_probe exactly once, then reply with its returned value. Do not invent it.');
      assert.equal(calls,1,harness);assert.ok(turn.events.some(event=>event.type==='tool.result'));
      const typed=await session.send('Return the previously retrieved value in the value field. Do not call the tool again.',{output:{type:z.object({value:z.literal(answer)})}});
      assert.equal(typed.value,answer);assert.equal(calls,1);
      assert.ok((await client.account.modelUsage(session.id)).reported_input_tokens>0);
      console.log(`${harness}: real provider, application callback, typed answer and usage passed`);
    } finally {await session.delete();}
  }
} finally {await client.close();}
