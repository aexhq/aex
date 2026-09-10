import { Aex, brainEnv } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { z } from "zod";

for (const name of ["AEX_API_KEY", "MODEL_API_KEY"]) {
  if (!process.env[name]) throw new Error(`${name} required`);
}
const aex = new Aex({ apiKey: process.env.AEX_API_KEY });
const session = await aex.sessions.create({
  model: { provider: "openai", name: "gpt-4.1-mini", apiKey: process.env.MODEL_API_KEY },
  agentloop: pi({ env: brainEnv({ name: "brain" }) }),
});
try {
  const person = await session.send("Ada is 37 years old. Extract her details.", {
    output: { type: z.object({ name: z.string(), age: z.number() }), maxRetries: 2 },
  });
  console.log(person);
} finally {
  await session.end();
  await session.delete();
}
