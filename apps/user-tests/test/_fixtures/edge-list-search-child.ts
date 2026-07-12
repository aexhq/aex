export function buildEdgeListSearchChildScript(body: string): string {
  return `
    import { Aex } from "@aexhq/sdk";
    const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
    const PROVIDER = process.env.PROVIDER;
    const PROVIDER_KEY = process.env.PROVIDER_KEY;
    const MODEL = process.env.MODEL;
    ${body}
  `;
}

export const EDGE_SESSION_DEBUG_BODY = String.raw`
  const lines = [];
  const debugClient = new Aex({
    baseUrl: process.env.AEX_API_URL,
    apiKey: process.env.AEX_API_KEY,
    debug: (line) => lines.push(line)
  });
  await debugClient.sessions.list({ limit: 1 });
  const joined = lines.join("\n");
  process.stdout.write(JSON.stringify({
    count: lines.length,
    leakedApiKey: joined.includes(process.env.AEX_API_KEY),
    leakedProviderKey: joined.includes(PROVIDER_KEY)
  }));
`;
