---
name: frame-vision-gate
description: |
  Caption an image/video frame with a Doubao vision model and decide whether the
  frame ACTUALLY DEPICTS named target nouns (frame-grounded verification), not
  merely mentions/relates to them. Use this whenever the task requires proving a
  candidate image is visually on-target (e.g. b-roll selection, evidence gating).
  Reads the Doubao key from DOUBAO_API_KEY supplied as a runtime secret.
---

# frame-vision-gate

This skill answers one question about one image: **"does the frame literally show
these things?"** It captions the pixels with a vision-language model (Doubao Seed
1.6 Vision on the ByteDance Ark API) and returns a per-noun depiction verdict the
agent can gate on.

## Prerequisites

The run must provide:

- `DOUBAO_API_KEY` in `environment.secrets`.
- Networking access to `ark.ap-southeast.bytepluses.com`, or the matching host
  for your Doubao account.

Example:

```ts
environment: {
  secrets: {
    DOUBAO_API_KEY: Secret.value(process.env.DOUBAO_API_KEY!)
  },
  networking: {
    mode: "limited",
    allowedHosts: ["ark.ap-southeast.bytepluses.com"]
  }
}
```

## How to call it

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
    "provider": "doubao",
    "model": "doubao-seed-1-6-vision-250815"
  }
}
```

### 2. Accept or reject the candidate

```bash
python skills/frame-vision-gate/verify_frame.py \
  --caption-json /workspace/.aex/caption.json \
  --need-type role_example \
  --visual-must-depict "an owlbear" \
  --visual-must-depict "a tabletop RPG miniature" \
  --out /workspace/.aex/verdict.json
```

`verify_frame.py` accepts only when every `--visual-must-depict` noun has a VLM
verdict `depicts:true` with `confidence >= 0.5`, the frame has a real caption,
and the need-type class is satisfied. It never substring-matches a title or URL.

## Pitfalls

- One image per call. To verify a video, extract keyframes first.
- Scale frames to roughly 480-960px before captioning. Full-res frames add cost
  without much extra signal.
- The model judges pixels only. Do not pass the candidate title into the caption
  prompt.
