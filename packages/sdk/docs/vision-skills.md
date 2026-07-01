---
title: Call a vision (or any model) API from a skill
---

# Call a vision (or any model) API from a skill

aex has no built-in vision tool. The agent's `provider`/`model` selects the
*reasoning* model — it is not an endpoint a skill can POST an image to mid-run.
To give a run image understanding (or to call any other model/HTTP API), ship a
**skill** that POSTs to the provider's OpenAI-compatible endpoint **through the
managed proxy**, with the key supplied on a `ProxyEndpoint.bearer(...)` instance.
The raw key never enters the container.

This is the same proxy described in `credentials.md` — this page is the worked
recipe for the model-API case, which has two wrinkles a plain JSON call does not:
the image rides as a **base64 data URL** in the request body, and that body is
large enough to need a raised `maxRequestBytes`.

The canonical, runnable example lives in the repo at
[`examples/vision-skill/`](../../../examples/vision-skill) (`SKILL.md`,
`caption_frame.py`, `verify_frame.py`, `run_with_vision_skill.mjs`). It
captions a frame with ByteDance Doubao Seed Vision (Ark) and returns a per-noun
"does the frame depict X?" verdict. Everything below is taken from it.

## 1. Declare the model endpoint as a proxy endpoint

The vision provider's API is just an HTTPS host. Declare it with
`ProxyEndpoint.bearer(...)`, which carries the key on the instance. The two
model-specific settings are `responseMode: "full"` (so the skill gets the upstream
JSON back) and a raised `maxRequestBytes` (so the base64 image fits):

```ts
import { Aex, Models, Skill, ProxyEndpoint } from "@aexhq/sdk";

const aex = new Aex({ apiToken: process.env.AEX_API_TOKEN! });

const doubaoArk = ProxyEndpoint.bearer({
  name: "doubao-ark",
  baseUrl: "https://ark.ap-southeast.bytepluses.com", // intl BytePlus gateway
  token: process.env.DOUBAO_API_KEY!,
  allowMethods: ["POST"],
  allowPathPrefixes: ["/api/v3/chat/completions"],
  maxRequestBytes: 2_000_000, // base64 image POSTs — see note below
  responseMode: "full",
  timeoutMs: 60_000
});

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "…read skills/frame-vision-gate/SKILL.md, then caption + verify the frame…",
  skills: [await Skill.fromPath("./vision-skill", { name: "frame-vision-gate" })],
  proxyEndpoints: [doubaoArk],
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

`Skill.fromPath("./vision-skill", …)` is resolved relative to the process CWD, so
run the script from the directory that *contains* `vision-skill/` (in the
repo, that is `examples/`). The same pattern works for OpenAI, Gemini's
OpenAI-compatible endpoint, or any other OpenAI-chat-shaped vision API — only
`baseUrl` and the path prefix change.

## 2. POST the image as a base64 data URL through the proxy

Inside the run, the skill builds the OpenAI-compatible chat-completions body. The
image is **base64-inlined as a data URL** in an `image_url` content part — it is
not uploaded:

```python
import base64, json
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
```

Write the body to a file and hand it to the mounted CLI with `--data @<file>`
(the mount has no execute bit, so invoke through `bun`; see `credentials.md`):

```python
import subprocess
body_path = "/workspace/.aex/_ark_request.json"
open(body_path, "w").write(json.dumps(request_body))

result = subprocess.run(
    ["bun", "/mnt/session/uploads/aex/aex", "proxy", "doubao-ark",
     "--method", "POST",
     "--path", "/api/v3/chat/completions",
     "--header", "content-type=application/json",
     "--data", f"@{body_path}",
     "--response-mode", "full"],
    capture_output=True, text=True, timeout=90,
)
```

In `--response-mode full` the CLI prints a `ProxyResponseEnvelope` on stdout. The
upstream JSON is **base64-encoded** in `upstreamBodyBase64`; an error instead
carries an `error` field. Unwrap it:

```python
envelope = json.loads(result.stdout)
if "error" in envelope:
    raise RuntimeError(f"proxy error: {envelope['error']}: {envelope['message']}")
upstream = json.loads(base64.b64decode(envelope["upstreamBodyBase64"]).decode())
content = upstream["choices"][0]["message"]["content"]  # the model's JSON answer
```

The key is injected by the hosted proxy on the outbound call; it never appears on disk in
the container or in the model's context.

## `maxRequestBytes` and timeout defaults

The per-endpoint `maxRequestBytes` default is **10 MiB** and the default timeout
is **5 minutes**. That fits typical base64 image/model POSTs without extra
configuration. If a body does exceed the cap, the proxy rejects it before any
upstream call with an explicit error naming the observed size, the configured
cap, and how to raise it:

> request body is 2400000 bytes, which exceeds this endpoint's maxRequestBytes
> (10485760). Raise the per-endpoint maxRequestBytes in the proxy endpoint policy …

Two ways to stay under the cap: raise `maxRequestBytes`, and/or scale frames
before captioning (`ffmpeg -i source.mp4 -vf fps=1,scale=960:-1 frame_%03d.jpg`)
so full-res frames do not add payload and model cost without useful signal.

## Notes

- **Host selection.** Use the provider endpoint that matches your account and
  declare it as the proxy endpoint `baseUrl`.
- **Keyless model hosts.** If the upstream takes no credential, declare the
  endpoint with `ProxyEndpoint.none(...)` (see `credentials.md`).
- **Response size.** `responseMode: "full"` is required to read the model's reply
  back. Leave `maxResponseBytes` at its default (`0` = unlimited, streamed) unless
  you want a truncation cap.
