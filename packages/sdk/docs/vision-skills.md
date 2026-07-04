---
title: Call a vision API from a skill
---

# Call a vision API from a skill

aex has no built-in vision tool. The agent's `provider` / `model` selects the
reasoning model for the run; if a skill needs image understanding mid-run, ship a
skill that calls the vision provider with normal HTTP and pass that provider key
as a runtime secret.

The runnable example lives at [`examples/vision-skill/`](../../../examples/vision-skill).
It captions a frame with ByteDance Doubao Seed Vision (Ark) and returns a
per-noun "does the frame depict X?" verdict.

## Submit the run

```ts
import { Aex, Models, Secret, Tools } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

const result = await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Read skills/frame-vision-gate/SKILL.md, then caption and verify the frame.",
  tools: [await Tools.fromSkillDir("./vision-skill", { name: "frame-vision-gate" })],
  environment: {
    secrets: {
      DOUBAO_API_KEY: Secret.value(process.env.DOUBAO_API_KEY!)
    },
    networking: {
      mode: "limited",
      allowedHosts: ["ark.ap-southeast.bytepluses.com"]
    }
  },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

console.log(result.runId, result.text);
```

`Tools.fromSkillDir("./vision-skill", ...)` is resolved relative to the process
CWD. Run the script from the directory that contains `vision-skill/` (in this
repo, `examples/`).

## Call the provider from the skill

Inside the run, the skill reads `DOUBAO_API_KEY` and makes an
OpenAI-compatible chat-completions request with Python's standard HTTP client.
The image is base64-inlined as a data URL in the request body:

```python
import base64, json, os, urllib.request

b64 = base64.b64encode(open("/workspace/files/frame.jpg", "rb").read()).decode()
request_body = {
    "model": "doubao-seed-1-6-vision-250815",
    "temperature": 0,
    "response_format": {"type": "json_object"},
    "messages": [
        {"role": "system", "content": "Describe only what the pixels show."},
        {"role": "user", "content": [
            {"type": "text", "text": "Does this frame depict an owlbear? Answer as JSON."},
            {"type": "image_url", "image_url": {"url": f"data:image/jpeg;base64,{b64}"}}
        ]}
    ]
}

req = urllib.request.Request(
    "https://ark.ap-southeast.bytepluses.com/api/v3/chat/completions",
    data=json.dumps(request_body).encode("utf-8"),
    headers={
        "Authorization": f"Bearer {os.environ['DOUBAO_API_KEY']}",
        "Content-Type": "application/json"
    },
    method="POST",
)
```

The same pattern works for OpenAI, Gemini's OpenAI-compatible endpoint, or any
other HTTPS model API. Put the key in `environment.secrets`, allow-list the host
when using limited networking, and use the provider's normal SDK or HTTP API.

## Payload size

Base64 images are larger than their source files. Scale frames before captioning
when possible, for example:

```bash
ffmpeg -i source.mp4 -vf fps=1,scale=960:-1 frame_%03d.jpg
```

This keeps upload size and model cost bounded without losing the signal most
vision models need.
