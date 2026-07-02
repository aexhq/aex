#!/usr/bin/env python3
"""Caption one image frame with Doubao Seed 1.6 Vision and return per-noun
depiction verdicts.

The run supplies DOUBAO_API_KEY through environment.secrets. This script makes a
normal HTTPS POST to the Ark OpenAI-compatible endpoint, so the run's networking
policy must allow that host.
"""
from __future__ import annotations

import argparse
import base64
import json
import mimetypes
import os
import re
import sys
import urllib.request
from pathlib import Path
from typing import Any

# Defaults match project-broll (core/shared/llm/doubao_json.py).
DEFAULT_VISION_MODEL = "doubao-seed-1-6-vision-250815"
ARK_PATH = "/api/v3/chat/completions"
DIRECT_BASE_URL = os.getenv("DOUBAO_BASE_URL", "https://ark.ap-southeast.bytepluses.com")


def main() -> None:
    p = argparse.ArgumentParser(description="Caption a frame with a Doubao VLM and emit per-noun depiction verdicts.")
    p.add_argument("--image", required=True)
    p.add_argument("--must-depict", action="append", default=[],
                   help="A visual noun the FRAME ITSELF must depict (repeatable).")
    p.add_argument("--model", default=DEFAULT_VISION_MODEL)
    p.add_argument("--out", required=True)
    args = p.parse_args()

    image_path = Path(args.image)
    try:
        facts = caption_image(image_path, args.must_depict, args.model)
        payload = {
            "schema": "broll_builder.caption_visual.v1",
            "ok": bool(facts.get("caption")),
            "image_path": image_path.as_posix(),
            "visual_facts": facts,
        }
    except Exception as exc:  # structured CLI failure, like the broll tools
        payload = {
            "schema": "broll_builder.caption_visual.v1",
            "ok": False,
            "image_path": image_path.as_posix(),
            "error": f"{type(exc).__name__}: {exc}",
        }

    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(args.out)
    if not payload["ok"]:
        sys.exit(1)


def caption_image(image_path: Path, must_depict: list[str], model: str) -> dict[str, Any]:
    mime = mimetypes.guess_type(image_path.name)[0] or "image/jpeg"
    data = base64.b64encode(image_path.read_bytes()).decode("ascii")
    # IDENTICAL request shape to broll's _caption_openai_compatible (Ark is
    # OpenAI-chat-compatible): system + (text prompt, image_url data: URL).
    request_body = {
        "model": model,
        "messages": [
            {"role": "system", "content": "Inspect visual evidence. Return JSON only."},
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": json.dumps(_prompt(must_depict), ensure_ascii=False)},
                    {"type": "image_url", "image_url": {"url": f"data:{mime};base64,{data}"}},
                ],
            },
        ],
        "temperature": 0,
        "max_tokens": 900,
        "response_format": {"type": "json_object"},
    }

    raw = _post_direct(request_body)
    text = raw["choices"][0]["message"]["content"]
    facts = _parse_caption_text(text)
    facts["provider"] = "doubao"
    facts["model"] = model
    facts["must_depict"] = must_depict
    _backfill_depicts_from_caption(facts, must_depict)
    return facts


# ---- transport -----------------------------------------------------------------

def _post_direct(request_body: dict[str, Any]) -> dict[str, Any]:
    api_key = (os.getenv("DOUBAO_API_KEY") or "").strip()
    if not api_key:
        raise RuntimeError("DOUBAO_API_KEY is not set; supply it via environment.secrets")
    req = urllib.request.Request(
        f"{DIRECT_BASE_URL.rstrip('/')}{ARK_PATH}",
        data=json.dumps(request_body).encode("utf-8"),
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        return json.loads(resp.read().decode("utf-8"))


# ---- prompt + parsing (ported from caption_visual.py) ---------------------------

def _prompt(must_depict: list[str]) -> dict[str, Any]:
    prompt: dict[str, Any] = {
        "task": "Inspect ONLY the attached image. Describe what is literally visible in the pixels.",
        "return_schema": {
            "caption": "one sentence about visible evidence",
            "visible_subjects": ["people, characters, props, readable text"],
            "scene_type": "actor_photo | role_clip | source_scene | bts_set | document_page | webpage | generic_explainer | unknown",
            "visual_intent_class": "actor_photo | role_clip | source_scene | bts_set | document_page | webpage | generic_explainer | unknown",
            "red_flags": ["why this might not be usable"],
            "direct_usefulness": 0.0,
        },
    }
    if must_depict:
        prompt["depiction_question"] = (
            "For each noun below, decide whether the image actually SHOWS that thing "
            "(it is visibly present in the frame), as opposed to merely mentioning, "
            "naming, listing, or relating to it (a title card, a stat block, a book "
            "cover, or a talking head ABOUT it does NOT show it). Judge from pixels only."
        )
        prompt["must_depict"] = must_depict
        prompt["return_schema"]["depicts"] = {
            noun: {"depicts": "true|false", "confidence": 0.0, "evidence": "what in the frame supports this"}
            for noun in must_depict
        }
    return prompt


def _parse_caption_text(text: str) -> dict[str, Any]:
    cleaned = text.strip()
    if cleaned.startswith("```"):
        cleaned = cleaned.strip("`")
        if cleaned.startswith("json"):
            cleaned = cleaned[4:]
    start, end = cleaned.find("{"), cleaned.rfind("}")
    if start >= 0 and end >= start:
        cleaned = cleaned[start:end + 1]
    try:
        parsed = json.loads(cleaned)
    except json.JSONDecodeError:
        parsed = {"caption": text[:1000].strip(), "red_flags": ["MODEL_RETURNED_NON_JSON"]}
    return _normalize(parsed if isinstance(parsed, dict) else {})


def _normalize(raw: dict[str, Any]) -> dict[str, Any]:
    caption = str(raw.get("caption") or "")
    return {
        "caption": caption,
        "visible_subjects": _list(raw.get("visible_subjects")),
        "scene_type": str(raw.get("scene_type") or ""),
        "visual_intent_class": str(raw.get("visual_intent_class") or raw.get("scene_type") or "unknown"),
        "red_flags": _list(raw.get("red_flags")),
        "direct_usefulness": _bounded(raw.get("direct_usefulness")),
        "depicts": _normalize_depicts(raw.get("depicts")),
    }


def _normalize_depicts(value: Any) -> dict[str, dict[str, Any]]:
    if not isinstance(value, dict):
        return {}
    out: dict[str, dict[str, Any]] = {}
    for noun, verdict in value.items():
        if isinstance(verdict, dict):
            out[str(noun)] = {
                "depicts": _as_bool(verdict.get("depicts")),
                "confidence": _bounded(verdict.get("confidence")),
                "evidence": str(verdict.get("evidence") or ""),
            }
        else:
            out[str(noun)] = {"depicts": _as_bool(verdict), "confidence": 0.0, "evidence": ""}
    return out


def _backfill_depicts_from_caption(facts: dict[str, Any], must_depict: list[str]) -> None:
    """Salvage a missing per-noun verdict when the caption text plainly affirms the
    noun (mirrors caption_visual.py B5b). Lower confidence, flagged source."""
    if not must_depict:
        return
    depicts = dict(facts.get("depicts") or {})
    caption_text = " ".join([str(facts.get("caption") or ""), " ".join(facts.get("visible_subjects") or [])]).lower()
    have = {k.lower() for k in depicts}
    for noun in must_depict:
        if noun.lower() in have:
            continue
        words = [w for w in re.findall(r"[a-z0-9]+", noun.lower()) if len(w) > 2 and w not in _STOP]
        if words and all(w in caption_text for w in words):
            depicts[noun] = {"depicts": True, "confidence": 0.6,
                             "evidence": "caption-grounded: noun terms present in the blind caption",
                             "source": "caption_backfill"}
    facts["depicts"] = depicts


_STOP = {"the", "and", "for", "with", "from", "face", "shot", "image", "photo", "scene"}


def _as_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    return str(value).strip().lower() in {"true", "yes", "1", "depicts", "present"}


def _list(value: Any) -> list[str]:
    if isinstance(value, str):
        return [value] if value.strip() else []
    if isinstance(value, list):
        return [str(v) for v in value if str(v).strip()]
    return []


def _bounded(value: Any) -> float:
    try:
        return max(0.0, min(1.0, round(float(value), 3)))
    except (TypeError, ValueError):
        return 0.0


if __name__ == "__main__":
    main()
