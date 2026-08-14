#!/usr/bin/env bash
# Local Autara readiness runner (mirrors .github/workflows/autara-readiness.yml).
#
# Usage:
#   ./scripts/readiness.sh testnet
#   ./scripts/readiness.sh mainnet-safe
#
# testnet:
#   Requires E2E_* mint authority key paths (or defaults used by e2e_flow) and
#   starts a local autara-pyth pusher like CI.
#
# mainnet-safe:
#   Requires E2E_SKIP_MINT=1, E2E_USER_KEY (pre-funded), and E2E_PROGRAM_ID /
#   E2E_MARKET / E2E_AUSD_MINT / E2E_ABTC_MINT / ORACLE_PROGRAM_ID.
#   Does not start a local pusher (assumes production pusher is live).
#
# Always runs:
#   - cargo test (lib / program / integration / socialize_loss)
#   - shared_loss_flow example (stage disposable market)
#   - e2e_flow example
#
# Optional:
#   PROBE_PUSHER_HEALTH=https://.../health ./scripts/readiness.sh testnet

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MODE="${1:-}"
if [[ "$MODE" != "testnet" && "$MODE" != "mainnet-safe" ]]; then
  echo "usage: $0 testnet|mainnet-safe" >&2
  exit 2
fi

LOG_DIR="${READINESS_LOG_DIR:-$ROOT/target/readiness-logs}"
mkdir -p "$LOG_DIR"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
SUMMARY="$LOG_DIR/summary-$MODE-$TS.md"

pass() { echo "PASS  $*"; echo "- [x] $*" >>"$SUMMARY"; }
fail() { echo "FAIL  $*" >&2; echo "- [ ] $* (FAILED)" >>"$SUMMARY"; exit 1; }

{
  echo "# Autara readiness — \`$MODE\`"
  echo
  echo "- started: \`$TS\`"
  echo "- commit: \`$(git rev-parse --short HEAD 2>/dev/null || echo unknown)\`"
  echo
  echo "## Results"
} >"$SUMMARY"

echo "== 1) Unit + integration tests =="
cargo test -p autara-lib --lib
cargo test -p autara-program --lib
cargo test -p autara-integration-tests -- --nocapture
cargo test -p autara-integration-tests socialize_loss -- --nocapture
pass "unit + integration (+ socialize_loss) tests"

echo "== 2) Shared loss live flow (stage disposable market) =="
SL_LOG="$LOG_DIR/shared_loss_flow-$TS.log"
cargo run -p autara-client --example shared_loss_flow 2>&1 | tee "$SL_LOG"
grep -q 'FINAL VERDICT: PASS' "$SL_LOG" || fail "shared_loss_flow FINAL VERDICT: PASS"
pass "shared_loss_flow"

echo "== 3) Configure e2e ($MODE) =="
PUSHER_PID=""
cleanup() {
  if [[ -n "${PUSHER_PID:-}" ]]; then
    kill "$PUSHER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

if [[ "$MODE" == "testnet" ]]; then
  export E2E_RPC="${E2E_RPC:-https://rpc.testnet.arch.network}"
  export E2E_NETWORK="${E2E_NETWORK:-testnet4}"
  export E2E_PROGRAM_ID="${E2E_PROGRAM_ID:-34cf72a92dd76322a42f13f99e51cf7c03221f4adbd4ee7e0c409c4161dfe20c}"
  export E2E_MARKET="${E2E_MARKET:-d8d679b946aafb22322f477cd5f196700f181aa3f712ca09e486fc77cedc0cce}"
  export E2E_AUSD_MINT="${E2E_AUSD_MINT:-8ec480c6e5458e7d37dc2a9f7d7d149a02d8182a38523b037905203ff36b71f6}"
  export E2E_ABTC_MINT="${E2E_ABTC_MINT:-627ecd24366c89314b12aa08a1b2fffc3890cb9cf64fb04fe3e95c7182b23dfb}"
  export ORACLE_PROGRAM_ID="${ORACLE_PROGRAM_ID:-8d24068aa026fd2e6ccca6e7b64a944b0e384df279b15f599ddd4a5285d592e8}"
  export ORACLE_FEEDS="${ORACLE_FEEDS:-0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43,0xeaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a}"
  if [[ -z "${E2E_USER_KEY:-}" ]]; then
    mkdir -p /tmp/autara-e2e
    openssl rand -hex 32 | tr -d '\n' > /tmp/autara-e2e/user.key
    export E2E_USER_KEY=/tmp/autara-e2e/user.key
  fi
  unset E2E_SKIP_MINT || true

  echo "== 3a) Start local Pyth pusher =="
  cargo build --release -p autara-pyth
  PUSHER_LOG="$LOG_DIR/pusher-$TS.log"
  : >"$PUSHER_LOG"
  ./target/release/autara-pyth \
    --rpc "$E2E_RPC" \
    --network testnet \
    --program-id "$ORACLE_PROGRAM_ID" \
    --feeds "$ORACLE_FEEDS" \
    >"$PUSHER_LOG" 2>&1 &
  PUSHER_PID=$!
  ok=0
  for i in $(seq 1 60); do
    if ! kill -0 "$PUSHER_PID" 2>/dev/null; then
      echo "Pusher exited early; log:" >&2
      cat "$PUSHER_LOG" >&2
      fail "local pusher stay alive"
    fi
    if grep -q "Sending" "$PUSHER_LOG"; then
      ok=1
      break
    fi
    sleep 1
  done
  [[ "$ok" == "1" ]] || fail "local pusher push within 60s"
  pass "local pusher started"
else
  export E2E_SKIP_MINT=1
  export E2E_RPC="${E2E_RPC:-https://rpc.mainnet.arch.network}"
  export E2E_NETWORK="${E2E_NETWORK:-mainnet}"
  for req in E2E_USER_KEY E2E_PROGRAM_ID E2E_MARKET E2E_AUSD_MINT E2E_ABTC_MINT ORACLE_PROGRAM_ID; do
    if [[ -z "${!req:-}" ]]; then
      echo "mainnet-safe requires $req in the environment" >&2
      fail "mainnet-safe env ($req)"
    fi
  done
  pass "mainnet-safe env present (E2E_SKIP_MINT=1)"
fi

echo "== 4) e2e lending flow =="
cargo build -p autara-client --example e2e_flow
E2E_LOG="$LOG_DIR/e2e_flow-$TS.log"
set +e
cargo run -p autara-client --example e2e_flow 2>&1 | tee "$E2E_LOG"
rc=${PIPESTATUS[0]}
set -e
grep -q 'FINAL VERDICT: PASS' "$E2E_LOG" || fail "e2e_flow FINAL VERDICT: PASS (exit=$rc)"
[[ "$rc" -eq 0 ]] || fail "e2e_flow exit code 0"
pass "e2e_flow"

if [[ -n "${PROBE_PUSHER_HEALTH:-}" ]]; then
  echo "== 5) Probe pusher /health =="
  curl -fsS --max-time 15 "$PROBE_PUSHER_HEALTH" >/dev/null
  pass "pusher /health ($PROBE_PUSHER_HEALTH)"
fi

{
  echo
  echo "## Verdict"
  echo
  echo "**READY** for upgrade dry-run on the intended network."
  echo
  echo "Next: GitHub Actions → \`autara-release\` (dry-run first), or"
  echo "\`autara-upgrade\` with dry_run=true."
} >>"$SUMMARY"

echo
echo "=============================="
echo "READINESS PASS ($MODE)"
echo "Summary: $SUMMARY"
echo "=============================="
cat "$SUMMARY"
