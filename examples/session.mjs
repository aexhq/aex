import { Brain, agentloop, brainEnv, component, hostEnv, tool } from "@aexhq/brain";
import { pathToFileURL } from "node:url";
import { z } from "zod";

for(const name of ["AEX_URL","AEX_API_KEY","MODEL_API_KEY","AGENTLOOP_FILE"]) {
  if(!process.env[name]) throw new Error(`${name} required`);
}
const brain=new Brain({baseUrl:process.env.AEX_URL,token:process.env.AEX_API_KEY});
const lookup=tool({name:"lookup",description:"Look up an application value",input:z.object({id:z.string()}),run:({id})=>({id,value:"example"})});
const session=await brain.sessions.create({
  model:{provider:"openai",name:"gpt-4.1-mini",apiKey:process.env.MODEL_API_KEY},
  agentloop:agentloop({implementation:component(pathToFileURL(process.env.AGENTLOOP_FILE))})({env:brainEnv({name:"brain"})}),
  tools:[lookup({env:hostEnv({name:"app"})})],
});
const host=await brain.register();
try {
  await session.send("Look up item 42");
  for await(const event of session.events()) console.log(event);
} finally {
  try { await session.end(); await session.delete(); }
  finally { host.pump.stop(); await host.pump.closed; }
}
