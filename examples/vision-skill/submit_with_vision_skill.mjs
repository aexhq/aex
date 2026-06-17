// Submit a run that mounts the frame-vision-gate skill and lets the agent
// caption/verify image frames with Doubao via the MANAGED PROXY. This is the
// secret-safe path and it works on the published @aexhq/sdk that ships
// `proxyEndpoints` + `secrets.proxyEndpointAuth` (no Secret/secretEnv needed).
//
// Env required: AEX_WORKSPACE_TOKEN, DOUBAO_API_KEY, ANTHROPIC_API_KEY (or your
// chosen run provider key). Optional: AEX_API_URL for a non-default plane.
import { AgentExecutor, RunModels, Skill, ProxyEndpoint, validateProxyAuth } from "@aexhq/sdk";

const aex = new AgentExecutor({
  apiToken: process.env.AEX_WORKSPACE_TOKEN,
  ...(process.env.AEX_API_URL ? { baseUrl: process.env.AEX_API_URL } : {})
});

// The Doubao Ark vision endpoint, reached through the managed proxy. The key is
// injected by the proxy and never enters the container.
const proxyEndpoints = [
  ProxyEndpoint.bearer({
    name: "doubao-ark",
    baseUrl: "https://ark.ap-southeast.bytepluses.com", // intl BytePlus gateway
    allowMethods: ["POST"],
    allowPathPrefixes: ["/api/v3/chat/completions"],
    maxRequestBytes: 2_000_000, // base64 image ~1.33x raw; raise above the 64KB default
    responseMode: "full",
    timeoutMs: 60_000
  })
];

const proxyEndpointAuth = [
  { name: "doubao-ark", value: { type: "bearer", token: process.env.DOUBAO_API_KEY } }
];

validateProxyAuth(proxyEndpoints, proxyEndpointAuth); // fail fast at submit time

const runId = await aex.submit({
  provider: "anthropic",
  model: RunModels.CLAUDE_HAIKU_4_5,
  prompt: [
    "You have a candidate image at /workspace/files/candidate_frame.jpg and must",
    "decide whether it actually depicts an owlbear (a tabletop-RPG creature).",
    "Read skills/frame-vision-gate/SKILL.md, then run caption_frame.py and",
    "verify_frame.py as it documents. Report the verdict JSON."
  ].join(" "),
  skills: [await Skill.fromPath("./vision-skill", { name: "frame-vision-gate" })],
  proxyEndpoints,
  secrets: {
    apiKey: process.env.ANTHROPIC_API_KEY,
    proxyEndpointAuth
  }
});

console.log("RUN_ID", runId);
