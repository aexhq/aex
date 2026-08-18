#!/usr/bin/env bash
# Regenerates everything derived from contracts/ (the single source of truth):
#   - normalises the schema JSON files (stable formatting, so diffs are meaningful)
#   - contracts/examples/**            (from tools/make-examples.py)
#   - crates/aex-contracts/src/{abi,session}.rs   (cargo-typify)
#   - packages/contracts/src/{abi,session,paths}.ts (json-schema-to-typescript, openapi-typescript)
# CI runs this and fails on any diff ("generated content is never hand-edited").
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== normalise schema formatting"
python tools/normalise-json.py contracts/abi/v1/abi.json contracts/abi/v1/tools/manifest.json contracts/session/v1/schemas.json contracts/control/v1/schemas.json

echo "== examples"
python tools/make-examples.py

echo "== rust (cargo-typify $(cargo typify --version | awk '{print $2}'))"
gen_rs() { # $1 schema, $2 out
  cargo typify "$1" --additional-derive PartialEq -o "$2"
  # typify emits crate-level lints as inner attributes; they are valid at module level, keep them.
  rustfmt --edition 2024 "$2"
}
gen_rs contracts/abi/v1/abi.json crates/aex-contracts/src/abi.rs
gen_rs contracts/session/v1/schemas.json crates/aex-contracts/src/session.rs
gen_rs contracts/control/v1/schemas.json crates/aex-contracts/src/control.rs

echo "== typescript"
npm run --silent --workspace @aex/contracts gen

echo "== tool manifest digest"
cargo run --quiet -p aex-contracts --bin manifest-digest > contracts/abi/v1/tools/manifest.digest.tmp
mv contracts/abi/v1/tools/manifest.digest.tmp contracts/abi/v1/tools/manifest.digest

echo "done"
