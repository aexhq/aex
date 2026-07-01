// Run a session that mounts the frame-vision-gate skill and lets the agent
// caption/verify image frames with Doubao via the MANAGED PROXY. This is the
// secret-safe path: the Doubao key rides on the ProxyEndpoint.bearer instance and
// is split into the vaulted secrets channel server-side (never in the container).
//
// Env required: AEX_API_TOKEN, DOUBAO_API_KEY, ANTHROPIC_API_KEY (or your
// chosen run provider key). Optional: AEX_API_URL for a non-default plane.
import { Aex, Models, Tools, ProxyEndpoint } from "@aexhq/sdk";

const aex = new Aex({
  apiToken: process.env.AEX_API_TOKEN,
  ...(process.env.AEX_API_URL ? { baseUrl: process.env.AEX_API_URL } : {})
});

// The Doubao Ark vision endpoint, reached through the managed proxy. The key is
// injected by the proxy and never enters the container.
const doubaoArk = ProxyEndpoint.bearer({
  name: "doubao-ark",
  baseUrl: "https://ark.ap-southeast.bytepluses.com", // intl BytePlus gateway
  token: process.env.DOUBAO_API_KEY,
  allowMethods: ["POST"],
  allowPathPrefixes: ["/api/v3/chat/completions"],
  maxRequestBytes: 2_000_000, // base64 image ~1.33x raw; mind the request-size cap (default 10 MiB)
  responseMode: "full",
  timeoutMs: 60_000
});

const result = await aex.run({
  provider: "anthropic",
  model: Models.CLAUDE_HAIKU_4_5,
  message: [
    "You have a candidate image at /workspace/files/candidate_frame.jpg and must",
    "decide whether it actually depicts an owlbear (a tabletop-RPG creature).",
    "Read skills/frame-vision-gate/SKILL.md, then run caption_frame.py and",
    "verify_frame.py as it documents. Report the verdict JSON."
  ].join(" "),
  tools: [await Tools.fromSkillDir("./vision-skill", { name: "frame-vision-gate" })],
  proxyEndpoints: [doubaoArk],
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY }
});

console.log("SESSION_ID", result.runId);
console.log(result.text);
