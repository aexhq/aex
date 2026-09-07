#!/usr/bin/env bash
set -euo pipefail
root=$(pwd)
brain_revision=$(sed -n 's/.*rev = "\([a-f0-9]*\)".*/\1/p' crates/aex-server/Cargo.toml)
test "${#brain_revision}" = 40
checkout="${AEX_TEST_BRAIN_CHECKOUT:-$root/artifacts/brain-$brain_revision}"
if [ ! -d "$checkout/.git" ] && [ ! -f "$checkout/.git" ]; then
  git clone https://github.com/aexhq/brain.git "$checkout"
  git -C "$checkout" checkout --detach "$brain_revision"
fi
test "$(git -C "$checkout" rev-parse HEAD)" = "$brain_revision"
cargo build --locked -p aex-server
(cd "$checkout" && cargo build --locked -p brain-server --bin brain -p brain-env --bin brain-env-worker)
rustup target add wasm32-wasip2
cargo build --locked --manifest-path "$checkout/examples/reference-agentloop/Cargo.toml" --release --target wasm32-wasip2
cargo build --locked --manifest-path "$checkout/tests/fixtures/diagnostic-tool/Cargo.toml" --release --target wasm32-wasip2
export AEX_TEST_SERVER="$root/target/debug/aex-server"
export BRAIN_TEST_SERVER="$checkout/target/debug/brain"
export BRAIN_TEST_WORKER="$checkout/target/debug/brain-env-worker"
export BRAIN_TEST_REFERENCE_AGENTLOOP="$checkout/examples/reference-agentloop/target/wasm32-wasip2/release/reference_agentloop.wasm"
export BRAIN_TEST_TOOL="$checkout/tests/fixtures/diagnostic-tool/target/wasm32-wasip2/release/diagnostic_tool.wasm"
npm run build
npm test
