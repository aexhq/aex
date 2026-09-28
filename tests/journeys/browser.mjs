import { Aex, clientBrowser, tool } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";
import { z } from "zod";

window.runBrowserJourney = async ({ baseUrl, access }) => {
  let connections = 0;
  let suspended;
  const sleeping = new Promise(resolve => { suspended = resolve; });
  const aex = new Aex({ baseUrl, clientAccess: access, connectionIdleTimeoutMs: 25,
    fetch: async (url, options) => {
      if (new URL(url).pathname.endsWith("/commands")) connections++;
      const response = await fetch(url, options);
      if (new URL(url).pathname.endsWith("/suspend") && (await response.clone().json()).suspended) suspended();
      return response;
    },
  });
  const lookup = tool({ name: "lookup", description: "Read selection", input: z.object({ id: z.string() }),
    run: ({ id }, ctx) => ctx.finish(`${document.querySelector("#selection").textContent}-${id}`) });
  try {
    const session = await aex.sessions.create({
      model: { provider: "vercel-ai-gateway", name: "test/journey" },
      agentloop: pi(),
      tools: [lookup({ env: clientBrowser({ name: "tab" }) })],
    });
    await session.send("look it up");
    await sleeping;
    await session.send("look it up again after idle");
    return { id: session.id, connections, transcript: await session.transcript() };
  } finally { await aex.close(); }
};
