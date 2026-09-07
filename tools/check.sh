#!/usr/bin/env bash
set -euo pipefail
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo run --locked -q -p aex-server -- contract --output docs/generated
git diff --exit-code -- docs/generated
npm ci --ignore-scripts
npm audit --audit-level=high
npm run gen --workspace @aexhq/sdk
git diff --exit-code -- packages/sdk/src/generated.ts
npm run test:sdk
npm run test:cli
python3 -m unittest discover -s tests -p '*_test.py'
