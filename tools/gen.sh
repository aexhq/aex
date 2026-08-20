#!/usr/bin/env bash
# Regenerate Aex-owned control-plane contracts. Neutral session and Brain↔Hand contracts are
# generated and published by aexhq/brain.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== normalise Aex schema formatting"
python tools/normalise-json.py contracts/control/v1/schemas.json

echo "== Aex examples"
python tools/make-examples.py

echo "== Rust control types"
cargo typify contracts/control/v1/schemas.json --additional-derive PartialEq \
  -o crates/aex-contracts/src/control.rs
rustfmt --edition 2024 crates/aex-contracts/src/control.rs

echo "== TypeScript control types"
npm run --silent --workspace @aexhq/contracts gen

echo "done"
