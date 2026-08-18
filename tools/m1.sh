#!/usr/bin/env bash
# The slice-4 gate stack, real wire: a local brain (real model via your key) behind the control
# plane with REAL Stripe payments (test mode unless you point it at a live key). Starts both
# servers, prints the CLI flow, cleans up on Ctrl+C.
#
# The CI form of the same flow (stub brain, fake payments) is crates/aex-control/tests/control_e2e.rs.
#
# Keys are sourced BY NAME from $AEX_ENV_FILE (default: ../aex-backup/.env.dev relative to this
# repo) and are never echoed.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$(pwd)"
BRAIN_REPO="${AEX_BRAIN_REPO:-$REPO/../brain}"
ENV_FILE="${AEX_ENV_FILE:-$REPO/../aex-backup/.env.dev}"
GATE_DIR="${AEX_M1_DIR:-$REPO/.m1-gate}"

pick() { # $1 var name -> prints value, never the name=value line
  grep "^$1=" "$ENV_FILE" | head -1 | cut -d= -f2- | tr -d '\r'
}

STRIPE_SECRET_KEY="$(pick STRIPE_SECRET_KEY)"
[ -n "$STRIPE_SECRET_KEY" ] || { echo "STRIPE_SECRET_KEY not found in $ENV_FILE"; exit 1; }
case "$STRIPE_SECRET_KEY" in
  sk_test_*) echo "stripe: test mode";;
  sk_live_*) echo "stripe: LIVE mode — real money. Ctrl+C now if unintended."; sleep 5;;
esac

echo "== build"
cargo build -q -p aex-control -p aex-cli
(cd "$BRAIN_REPO" && cargo build -q --bin brain)

mkdir -p "$GATE_DIR"
BRAIN_TOKEN="m1-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"

echo "== brain (AEX_MODE=local) on 127.0.0.1:8700"
env -u AWS_PROFILE -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN \
  AEX_MODE=local AEX_API_TOKEN="$BRAIN_TOKEN" AEX_DATA_DIR="$GATE_DIR/brain-data" \
  AEX_LISTEN=127.0.0.1:8700 \
  "$BRAIN_REPO/target/debug/brain" &
BRAIN_PID=$!

echo "== control plane on 127.0.0.1:8600 (payments: stripe)"
AEX_BRAIN_URL=http://127.0.0.1:8700 AEX_BRAIN_TOKEN="$BRAIN_TOKEN" \
  AEX_CONTROL_DB="$GATE_DIR/control.db" AEX_CONTROL_LISTEN=127.0.0.1:8600 \
  AEX_PAYMENTS=stripe STRIPE_SECRET_KEY="$STRIPE_SECRET_KEY" \
  ./target/debug/aex-control &
CONTROL_PID=$!

trap 'kill $CONTROL_PID $BRAIN_PID 2>/dev/null || true' EXIT INT TERM
sleep 1

cat <<'EOF'

The stack is up. The stranger flow (use AEX_CONFIG to keep gate credentials separate):

  export AEX_CONFIG=.m1-gate/cli-config.json
  aex signup --email you@example.com
  aex topup --usd 10            # pay at the printed Stripe checkout URL (test card 4242...)
  aex keys create --name gate
  aex session new --provider anthropic --model <model> --key-env <YOUR_KEY_ENV> [--model-base-url <gateway>]
  aex session send <ses_...> "write hello.txt with one line, then cat it"
  aex usage                     # the bill: rated compute + storage meters
  aex session end <ses_...> && aex session delete <ses_...>

Ctrl+C stops both servers.
EOF
wait $CONTROL_PID
