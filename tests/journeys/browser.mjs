import { Aex, brainEnv, clientBrowser, tool } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { z } from "zod";

window.runBrowserJourney = async ({ baseUrl, access }) => {
  const aex = new Aex({ baseUrl, clientAccess: access });
  const lookup = tool({ name: "lookup", description: "Read selection", input: z.object({ id: z.string() }),
    run: ({ id }, ctx) => ctx.finish(`${document.querySelector("#selection").textContent}-${id}`) });
  try {
    const session = await aex.sessions.create({
      model: { provider: "vercel-ai-gateway", name: "test/journey" },
      agentloop: pi({ env: brainEnv({ name: "brain" }) }),
      tools: [lookup({ env: clientBrowser({ name: "tab" }) })],
    });
    await session.send("look it up");
    return { id: session.id, transcript: await session.transcript() };
  } finally { await aex.close(); }
};
