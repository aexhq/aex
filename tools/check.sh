#!/usr/bin/env bash
set -euo pipefail
(cd runtime && go mod download && go test ./... && go vet ./...)
node tools/brain-contract.mjs --check
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo run --locked -q -p aex-server -- contract --output docs/generated
git diff --exit-code -- docs/generated
npm ci --ignore-scripts
npm audit --audit-level=high
node --test tools/*.test.mjs services/tools/*.test.mjs services/media/server.test.mjs
npm run gen --workspace @aexhq/sdk
git diff --exit-code -- packages/sdk/src/generated.ts
npm run test:sdk
npm run test:cli
python3 -m unittest discover -s tests -p '*_test.py'
