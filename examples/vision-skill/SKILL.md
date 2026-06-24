---
name: frame-vision-gate
description: |
  Caption an image/video frame with a Doubao vision model and decide whether the
  frame ACTUALLY DEPICTS named target nouns (frame-grounded verification), not
  merely mentions/relates to them. Use this whenever the task requires proving a
  candidate image is visually on-target (e.g. b-roll selection, evidence gating).
  Reads no secret from disk: the Doubao Ark key is injected by the managed proxy.
---

# frame-vision-gate — does this frame depict X?

This skill answers one question about one image: **"does the frame literally show
these things?"** It captions the pixels with a vision-language model (Doubao Seed
1.6 Vision on the ByteDance Ark API) and returns a per-noun depiction verdict the
agent can gate on. It replaces substring/title matching — a stat block that
*mentions* "owlbear", a book cover, or a talking-head *about* X never passes as a
shot of X.

## When to use

- You have a candidate image on the local filesystem (downloaded image, or a
  keyframe you extracted from a video with `ffmpeg`).
- You need a hard accept/reject on "the frame visibly depicts <nouns>".

## Prerequisites the run must declare

This skill calls the Doubao Ark vision endpoint **through the aex managed proxy**,
so the API key never touches the container. At submit time the caller MUST declare:

1. A proxy endpoint named `doubao-ark`, declared with `ProxyEndpoint.bearer(...)`:

   ```ts
   import { ProxyEndpoint } from "@aexhq/sdk";

   const proxyEndpoints = [
     ProxyEndpoint.bearer({
       name: "doubao-ark",
       baseUrl: "https://ark.ap-southeast.bytepluses.com", // intl BytePlus gateway
       allowMethods: ["POST"],
       allowPathPrefixes: ["/api/v3/chat/completions"],
       maxRequestBytes: 2_000_000, // base64 image is ~1.33x raw; raise above the 64KB default
       responseMode: "full",
       timeoutMs: 60_000
     })
   ];
   ```

2. The matching auth value in `secrets.proxyEndpointAuth`:

   ```ts
   const proxyEndpointAuth = [{
     name: "doubao-ark",
     value: { type: "bearer", token: process.env.DOUBAO_API_KEY! }
   }] as const;
   ```

(China gateway: set `baseUrl` to `https://ark.cn-beijing.volces.com` and declare
`doubao-ark` against it — same path prefix. Note the China host's reachability
from the platform egress is currently unverified; prefer the BytePlus host.)

The skill auto-detects the endpoint and falls back to **direct egress** (a plain
HTTPS POST from `python`) when the proxy endpoint is absent — in that mode the run
must instead expose the key as `secretEnv: { DOUBAO_API_KEY: Secret.value(...) }`
and allow-list the Ark host under `environment.networking` (see README).

## How to call it

The skill ships at `skills/frame-vision-gate/`. Run the two steps with `bash`:

### 1. Caption + per-noun depiction verdict

```bash
python skills/frame-vision-gate/caption_frame.py \
  --image /workspace/files/candidate_frame.jpg \
  --must-depict "an owlbear" \
  --must-depict "a tabletop RPG miniature" \
  --out /workspace/.aex/caption.json
```

Writes a JSON `visual_facts` object:

```json
{
  "ok": true,
  "image_path": "...",
  "visual_facts": {
    "caption": "a feathered bear-owl creature miniature on a grid map",
    "visible_subjects": ["owlbear miniature", "battle grid"],
    "scene_type": "role_clip",
    "visual_intent_class": "role_clip",
    "direct_usefulness": 0.8,
    "depicts": {
      "an owlbear": {"depicts": true, "confidence": 0.9, "evidence": "feathered bear-owl in frame"},
      "a tabletop RPG miniature": {"depicts": true, "confidence": 0.85, "evidence": "painted mini on grid"}
    },
    "provider": "doubao", "model": "doubao-seed-1-6-vision-250815"
  }
}
```

### 2. Accept / reject the candidate

```bash
python skills/frame-vision-gate/verify_frame.py \
  --caption-json /workspace/.aex/caption.json \
  --need-type role_example \
  --visual-must-depict "an owlbear" \
  --visual-must-depict "a tabletop RPG miniature" \
  --out /workspace/.aex/verdict.json
```

`verify_frame.py` accepts only when EVERY `--visual-must-depict` noun has a VLM
verdict `depicts:true` with `confidence >= 0.5` (it never substring-matches the
title/url), the frame has a real caption, and the need-type class is satisfied.
Output: `{ "accepted": true|false, "frame_grounded": true|false,
"rejection_reasons": [...], "score": <float> }`.

## Pitfalls

- **One image per call.** To verify a video, extract keyframes first
  (`ffmpeg -i source.mp4 -vf fps=1,scale=480:-1 frame_%03d.jpg`) and caption each.
- **Scale frames to ~480px** before captioning. It keeps the base64 payload under
  the proxy `maxRequestBytes` and is what the model needs — full-res adds cost,
  not signal.
- **`depicts:true` from the VLM governs**, not the title. If a candidate's title
  says "Owlbear" but the frame is a title card, `verify_frame.py` rejects it with
  `FRAME_DOES_NOT_DEPICT:an owlbear`. That is the point.
- The model judges PIXELS ONLY. Do not pass the candidate title into the caption
  prompt — the script keeps the caption blind by design.
