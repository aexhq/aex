export function buildEdgeListSearchChildScript(body: string): string {
  return `
    import { Aex } from "@aexhq/sdk";
    const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
    const MODEL = process.env.MODEL;
    const knownSecrets = [process.env.AEX_API_KEY]
      .filter((value) => typeof value === "string" && value.length > 0);
    function serialized(value) {
      try { return JSON.stringify(value) || ""; } catch { return ""; }
    }
    function containsKnownSecret(value) {
      const text = serialized(value);
      return knownSecrets.some((secret) => text.includes(secret));
    }
    function redactKnownSecrets(value) {
      let text = serialized(value);
      for (const secret of knownSecrets) text = text.split(secret).join("[REDACTED]");
      return JSON.parse(text);
    }
    function printSafe(value) {
      const leakedKeyAnywhere = containsKnownSecret(value);
      const safe = redactKnownSecrets({ ...value, leakedKeyAnywhere });
      process.stdout.write(JSON.stringify(safe));
    }
    await (async () => {
      ${body}
    })();
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
  printSafe({
    count: lines.length,
    leakedApiKey: joined.includes(process.env.AEX_API_KEY)
  });
`;
