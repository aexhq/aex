#!/usr/bin/env python3
"""Accept/reject a captioned frame against a visual need. Ported from project-broll's
broll_builder/tools/verify_candidate.py, reduced to the frame-grounded gate: the
inspected frame must actually DEPICT every named noun (per the VLM's per-noun
verdict from caption_frame.py), never a substring/title match.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

MIN_DEPICT_CONFIDENCE = 0.5  # same threshold as verify_candidate.py

# Which inspected-frame classes satisfy a requested need_type (type-level policy,
# not a per-item allowlist) — same table as verify_candidate.py.
NEED_TYPE_VISUAL_CLASSES: dict[str, set[str]] = {
    "role_example": {"role_clip", "source_scene", "actor_photo", "bts_set"},
    "source_clip": {"source_scene", "role_clip", "actor_photo", "bts_set"},
    "actor_photo": {"actor_photo", "role_clip", "source_scene", "bts_set"},
    "bts_set": {"bts_set", "role_clip", "source_scene"},
    "document_page": {"document_page"},
}


def main() -> None:
    p = argparse.ArgumentParser(description="Verify one captioned frame against a visual need.")
    p.add_argument("--caption-json", required=True, help="Output of caption_frame.py.")
    p.add_argument("--need-type", required=True,
                   choices=["source_clip", "actor_photo", "role_example", "bts_set", "document_page"])
    p.add_argument("--visual-must-depict", action="append", default=[])
    p.add_argument("--out", required=True)
    args = p.parse_args()

    payload = json.loads(Path(args.caption_json).read_text(encoding="utf-8"))
    facts = payload.get("visual_facts") if isinstance(payload.get("visual_facts"), dict) else {}
    result = verify(facts, args.need_type, args.visual_must_depict)

    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(args.out)


def verify(facts: dict[str, Any], need_type: str, visual_must_depict: list[str]) -> dict[str, Any]:
    rejected: list[str] = []
    score = 0.0

    caption = str(facts.get("caption") or "")
    if not caption:
        rejected.extend(["NO_VISUAL_CAPTION", "NO_VISUAL_PROOF"])

    frame_text = " ".join([caption, " ".join(facts.get("visible_subjects") or [])]).lower()

    # Frame-grounded gate: every named noun must have a VLM verdict depicts:true with
    # confidence >= MIN. No substring match against a title/url can stand in for this.
    frame_grounded = False
    if visual_must_depict:
        depicts = facts.get("depicts") if isinstance(facts.get("depicts"), dict) else {}
        depicts_ci = {str(k).lower(): v for k, v in depicts.items()}
        depicted = 0
        weak = False
        for term in visual_must_depict:
            verdict = depicts_ci.get(term.lower())
            if verdict is not None:
                if verdict.get("depicts") and float(verdict.get("confidence") or 0.0) >= MIN_DEPICT_CONFIDENCE:
                    score += 3.0
                    depicted += 1
                else:
                    rejected.append(f"FRAME_DOES_NOT_DEPICT:{term}")
            elif term.lower() in frame_text:
                score += 1.0
                depicted += 1
                weak = True
                rejected.append(f"WEAK_FRAME_EVIDENCE:{term}")
            else:
                rejected.append(f"FRAME_DOES_NOT_DEPICT:{term}")
        frame_grounded = bool(caption) and depicted == len(visual_must_depict) and not weak

    # need_type class gate: any adjacent depiction class satisfies the need; a
    # confirmed frame-grounded subject satisfies any on-screen (non-citation) need.
    visual_class = str(facts.get("visual_intent_class") or "").lower()
    if need_type in NEED_TYPE_VISUAL_CLASSES:
        allowed = NEED_TYPE_VISUAL_CLASSES[need_type]
        depiction_ok = frame_grounded and need_type in {"actor_photo", "role_example", "source_clip", "bts_set"}
        if visual_class in allowed or depiction_ok:
            score += 4.0
        else:
            rejected.append(f"NOT_{need_type.upper()}_VISUAL")

    try:
        score += float(facts.get("direct_usefulness")) * 2.0
    except (TypeError, ValueError):
        pass

    accepted = not rejected and score >= 4.0
    return {
        "schema": "broll_builder.verify_candidate.v1",
        "accepted": accepted,
        "need_type": need_type,
        "score": round(score, 3),
        "frame_grounded": frame_grounded,
        "visual_must_depict": visual_must_depict,
        "rejection_reasons": rejected,
    }


if __name__ == "__main__":
    main()
