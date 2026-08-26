#!/usr/bin/env python3
"""Regenerate the hosted model catalog from models.dev.

Aex decides which provider names it serves on a customer's behalf, so the catalog is Aex's, not
Brain's: it maps a provider to the wire dialect Brain must speak, the endpoint that speaks it, and
each model's context window. Never hand-edit crates/aex-control/model-catalog.json — a fetch,
shape, or coverage failure exits non-zero and leaves the committed catalog untouched, so no build
can inherit a silently stale or gutted catalog.
"""

from __future__ import annotations

import collections
import datetime
import json
import pathlib
import sys
import urllib.request

SOURCE = "https://models.dev/api.json"
ROOT = pathlib.Path(__file__).resolve().parent.parent
TARGET = ROOT / "crates" / "aex-control" / "model-catalog.json"

# Brain seals a context window inside the session contract's bounds; a model outside them can
# never be admitted, so it is not catalog knowledge.
MIN_CONTEXT_WINDOW_TOKENS = 8192
MAX_CONTEXT_WINDOW_TOKENS = 2_000_000

# Which shape a provider speaks, keyed by the client package models.dev records for it. Upstream
# publishes an endpoint for the providers exposing a compatible HTTP API and omits it for the two
# vendors whose own endpoint is well known, which is why both tables are needed. Every endpoint is
# a full API root, version segment included: Brain appends only the operation path.
DIALECTS = {
    "@ai-sdk/anthropic": "anthropic",
    "@ai-sdk/gateway": "openai",
    "@ai-sdk/openai": "openai",
    "@ai-sdk/openai-compatible": "openai",
    "@openrouter/ai-sdk-provider": "openai",
}
VENDOR_ENDPOINTS = {
    "anthropic": "https://api.anthropic.com/v1",
    "openai": "https://api.openai.com/v1",
}
# A syntactically valid but gutted upstream document must fail the run, not publish a catalog that
# silently lost a dialect or the providers that define it.
REQUIRED = {
    "anthropic": ("anthropic", VENDOR_ENDPOINTS["anthropic"]),
    "openai": ("openai", VENDOR_ENDPOINTS["openai"]),
    "openrouter": ("openai", "https://openrouter.ai/api/v1"),
}


def fetch() -> dict:
    # models.dev refuses the default library agent; identify the generator instead.
    request = urllib.request.Request(
        SOURCE, headers={"user-agent": "aex-model-catalog (+https://github.com/aexhq/aex)"}
    )
    with urllib.request.urlopen(request, timeout=120) as response:
        if response.status != 200:
            sys.exit(f"{SOURCE} returned HTTP {response.status}")
        document = json.loads(response.read().decode("utf8"))
    if not isinstance(document, dict) or not document:
        sys.exit(f"{SOURCE} did not return a provider document")
    return document


def models_of(provider: dict) -> dict[str, int]:
    models = {}
    for model_id, model in sorted(provider.get("models", {}).items()):
        if not isinstance(model, dict):
            continue
        window = (model.get("limit") or {}).get("context")
        output = (model.get("modalities") or {}).get("output")
        if not isinstance(window, int) or isinstance(window, bool):
            continue
        if not MIN_CONTEXT_WINDOW_TOKENS <= window <= MAX_CONTEXT_WINDOW_TOKENS:
            continue
        if not isinstance(output, list) or "text" not in output:
            continue
        models[model_id] = window
    return models


def main() -> None:
    document = fetch()
    providers = collections.OrderedDict()
    for provider_id, provider in sorted(document.items()):
        if not isinstance(provider, dict):
            continue
        dialect = DIALECTS.get(provider.get("npm"))
        if dialect is None:
            # No official dialect speaks to this provider, so Aex cannot resolve its name.
            continue
        models = models_of(provider)
        if not models:
            continue
        endpoint = provider.get("api") or VENDOR_ENDPOINTS.get(provider_id)
        entry = collections.OrderedDict(dialect=dialect)
        if isinstance(endpoint, str):
            entry["base_url"] = endpoint
        entry["models"] = models
        providers[provider_id] = entry

    for provider_id, (dialect, endpoint) in REQUIRED.items():
        entry = providers.get(provider_id)
        if entry is None:
            sys.exit(f"{SOURCE} carries no usable {provider_id} models")
        if entry["dialect"] != dialect or entry.get("base_url") != endpoint:
            sys.exit(
                f"{provider_id} resolved to {entry['dialect']}/{entry.get('base_url')}; "
                f"expected {dialect}/{endpoint}"
            )

    catalog = collections.OrderedDict(
        source=SOURCE,
        generated=datetime.datetime.now(datetime.UTC).date().isoformat(),
        providers=providers,
    )
    TARGET.write_text(f"{json.dumps(catalog, indent=1)}\n", encoding="utf8")
    served = sum(1 for entry in providers.values() if "base_url" in entry)
    models = sum(len(entry["models"]) for entry in providers.values())
    print(
        f"hosted model catalog from {SOURCE}: {len(providers)} providers "
        f"({served} with a published endpoint), {models} models"
    )


if __name__ == "__main__":
    main()
