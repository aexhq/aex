// SessionRecord a session that mounts the frame-vision-gate skill and lets the agent
// caption/verify image frames with Doubao. The Doubao key is passed as a runtime
// secret and the skill makes a normal HTTPS call to the provider.
//
// Env required: AEX_API_KEY, DOUBAO_API_KEY, ANTHROPIC_API_KEY (or your
// chosen session provider key). Optional: AEX_API_URL for a non-default plane.
import { Aex, Models, Secret, Tools } from "@aexhq/sdk";

const aex = new Aex({
  apiKey: process.env.AEX_API_KEY,
  ...(process.env.AEX_API_URL ? { baseUrl: process.env.AEX_API_URL } : {})
});

const result = await aex.start({
  provider: "anthropic",
  model: Models.CLAUDE_HAIKU_4_5,
  message: [
    "You have a candidate image at /workspace/files/candidate_frame.jpg and must",
    "decide whether it actually depicts an owlbear (a tabletop-RPG creature).",
    "Read skills/frame-vision-gate/SKILL.md, then run caption_frame.py and",
    "verify_frame.py as it documents. Report the verdict JSON."
  ].join(" "),
  tools: [await Tools.fromSkillDir("./vision-skill", { name: "frame-vision-gate" })],
  environment: {
    secrets: {
      DOUBAO_API_KEY: Secret.value(process.env.DOUBAO_API_KEY)
    },
    networking: {
      mode: "limited",
      allowedHosts: ["ark.ap-southeast.bytepluses.com"]
    }
  },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY }
});

console.log("SESSION_ID", result.sessionId);
console.log(result.text);
