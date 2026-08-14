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

# keys/ is gitignored since #44, but autara-client still resolves the stage
# program, oracle and admin from those paths. Without them every integration
# test panics identically inside the fixture, which is a confusing way to learn
# your working copy is missing key material.
MISSING_KEYS=()
for k in autara-stage autara-pyth-stage autara-admin-stage; do
  [[ -s "keys/$k.key" ]] || MISSING_KEYS+=("keys/$k.key")
done
if ((${#MISSING_KEYS[@]})); then
  echo "Missing stage key material required by the integration suite:" >&2
  printf '  %s\n' "${MISSING_KEYS[@]}" >&2
  echo >&2
  echo "Restore your local copies, or recover them from git history (they were" >&2
  echo "untracked, not purged). See 'Restore stage keys' in docs/key-hygiene.md:" >&2
  echo >&2
  echo "  REF=\$(git rev-list -1 HEAD -- keys/autara-stage.key)^   # ^ = before deletion" >&2
  echo "  mkdir -p keys && for k in autara-stage autara-pyth-stage autara-admin-stage; do" >&2
  echo "    git show \"\$REF:keys/\$k.key\" > \"keys/\$k.key\" && chmod 600 \"keys/\$k.key\"" >&2
  echo "  done" >&2
  exit 2
fi

{
  echo "# Autara readiness — \`$MODE\`"
  echo
  echo "- started: \`$TS\`"
  echo "- commit: \`$(git rev-parse --short HEAD 2>/dev/null || echo unknown)\`"
  echo
  echo "## Results"
} >"$SUMMARY"

echo "== 1a) Unit tests (offline) =="
cargo test -p autara-lib --lib
cargo test -p autara-program --lib
pass "unit tests (autara-lib, autara-program)"

# The integration suite runs against LIVE Arch testnet, so cases intermittently
# fail while faucet-funded accounts / fresh markets propagate on the RPC node.
# Retry them like `make program-test` does.
# Retry count and backoff come from .config/nextest.toml; passing --retries here
# would override the profile and drop the backoff.
echo "== 1b) Integration tests (live testnet) =="
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run --no-fail-fast -p autara-integration-tests
else
  echo "cargo-nextest not found (install: curl -LsSf https://get.nexte.st/latest/mac | tar zxf - -C \"\${CARGO_HOME:-\$HOME/.cargo}/bin\")"
  echo "Falling back to cargo test WITHOUT retries; live-testnet flakes are likely."
  cargo test -p autara-integration-tests -- --nocapture
fi
pass "integration tests (incl. socialize_loss, capital_sweep)"

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
  # aUSD/aBTC market on the live stage deployment; the previous defaults pointed
  # at a testnet deployment that no longer exists.
  # Source of truth: deployments/testnet-ausd-abtc.json (PR #38).
  export E2E_PROGRAM_ID="${E2E_PROGRAM_ID:-53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192}"
  export E2E_MARKET="${E2E_MARKET:-9a5a237ddb156c367952ea3562ab3d05f3cdaf0e9bf6ba4fb7b76e233e181f53}"
  export E2E_AUSD_MINT="${E2E_AUSD_MINT:-55c6cee38a31732e2dad821ab1c38f902a7c51efaefb3641d51f3485c4617a45}"
  export E2E_ABTC_MINT="${E2E_ABTC_MINT:-1d46e0dd87393236e4e01252439f46dcbaec7c2255d1fd734e61771a00e8f4e9}"
  export ORACLE_PROGRAM_ID="${ORACLE_PROGRAM_ID:-eee682c27db375bebbc17ed9a76aaa935c8b72bc7de50d736f03e2dfbed84b15}"
  export ORACLE_FEEDS="${ORACLE_FEEDS:-0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43,0xeaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a}"
  if [[ -z "${E2E_USER_KEY:-}" ]]; then
    mkdir -p /tmp/autara-e2e
    openssl rand -hex 32 | tr -d '\n' > /tmp/autara-e2e/user.key
    export E2E_USER_KEY=/tmp/autara-e2e/user.key
  fi
  unset E2E_SKIP_MINT || true

  ./scripts/check-e2e-targets.sh || fail "e2e targets exist on-chain"

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
  ./scripts/check-e2e-targets.sh || fail "e2e targets exist on-chain"
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
