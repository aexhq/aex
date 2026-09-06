// AEX_API_KEY and VERCEL_AI_GATEWAY_API_KEY are required.
import { Aex, brainEnv } from "@aexhq/sdk";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read, write } from "@aexhq/tools";

const required = (name) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
};

const aex = new Aex({ apiKey: required("AEX_API_KEY") });
const workspace = awsMicroVm({ name: "sandbox", url: required("ENVIRONMENT_URL"), token: required("ENVIRONMENT_TOKEN"), region: "eu-west-2" });
const session = await aex.sessions.create({
  model: {
    provider: "vercel-ai-gateway",
    name: process.env.MODEL_NAME ?? "openai/gpt-5-mini",
    apiKey: required("VERCEL_AI_GATEWAY_API_KEY"),
  },
  agentloop: pi({ env: brainEnv({ name: "brain" }) }),
  system: "Work carefully and verify changes.",
  tools: [bash({ env: workspace }), read({ env: workspace }), write({ env: workspace })],
});

console.log(`created ${session.id}`);
await session.send(
  "Run `uname -a` and `date -u`, write both lines to /workspace/notes.txt, then summarize them.",
);
for await (const event of session.events()) {
  console.log(event.sequence, event.type, event.data);
}
await session.end();
await session.delete();
