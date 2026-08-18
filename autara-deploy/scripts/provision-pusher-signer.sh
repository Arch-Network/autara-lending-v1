#!/usr/bin/env bash
#
# provision-pusher-signer.sh — create a stable Autara oracle pusher signer for
# Railway (SIGNER_KEY_B64), optionally fund it from the testnet faucet.
#
# Required before the testnet oracle legacy-feed migrate: the first successful
# post-upgrade push binds feed authority to this key. A throwaway faucet key
# will strand the feeds after the container restarts.
#
# SAFETY:
#   - Writes the secret into a gitignored key dir (mode 0600).
#   - NEVER prints the secret or SIGNER_KEY_B64 to stdout by default.
#   - Writes base64 to a sibling *.b64 file (mode 0600) and can copy it to the
#     macOS clipboard with --copy.
#
# USAGE:
#   ./autara-deploy/scripts/provision-pusher-signer.sh [--network testnet] [options]
#
# OPTIONS:
#   --network <name>   testnet (default) | mainnet (generate only; no faucet)
#   --out-dir <dir>    key dir (default: autara-deploy/.keys-<network>)
#   --force            overwrite an existing pusher.key
#   --fund             faucet-fund on testnet/localnet (refused on mainnet)
#   --copy             copy SIGNER_KEY_B64 to the clipboard (pbcopy) without printing it
#   --print-b64        print SIGNER_KEY_B64 to stdout (use only on a private terminal)
#   --help             show this help
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

NETWORK="testnet"
OUT_DIR=""
FORCE=0
FUND=0
COPY=0
PRINT_B64=0

die() { echo "error: $*" >&2; exit 1; }

usage() { sed -n '3,28p' "$0" | sed 's/^# \?//'; }

while [ $# -gt 0 ]; do
  case "$1" in
    --network)   NETWORK="${2:?}"; shift 2 ;;
    --out-dir)   OUT_DIR="${2:?}"; shift 2 ;;
    --force)     FORCE=1; shift ;;
    --fund)      FUND=1; shift ;;
    --copy)      COPY=1; shift ;;
    --print-b64) PRINT_B64=1; shift ;;
    --help|-h)   usage; exit 0 ;;
    *)           die "unknown argument: $1 (see --help)" ;;
  esac
done

case "$NETWORK" in
  testnet|mainnet) ;;
  *) die "unsupported --network '$NETWORK' (use testnet|mainnet)" ;;
esac

KEY_DIR="${OUT_DIR:-autara-deploy/.keys-$NETWORK}"
KEY_PATH="$REPO_ROOT/$KEY_DIR/pusher.key"
B64_PATH="$REPO_ROOT/$KEY_DIR/pusher.signer.b64"

mkdir -p "$REPO_ROOT/$KEY_DIR"
if ! ( cd "$REPO_ROOT" && git check-ignore -q "$KEY_DIR" ); then
  die "refusing to write keys into '$KEY_DIR': it is NOT gitignored"
fi

if [ -e "$KEY_PATH" ]; then
  if [ "$FORCE" -eq 1 ]; then
    rm -f "$KEY_PATH" "$B64_PATH"
  else
    echo "reusing existing $KEY_DIR/pusher.key (pass --force to regenerate)" >&2
  fi
fi

PUBKEY=""
if [ ! -e "$KEY_PATH" ]; then
  # keygen uses bitcoin Network names; Arch testnet maps to "testnet".
  KEYGEN_NETWORK="$NETWORK"
  [ "$NETWORK" = "mainnet" ] && KEYGEN_NETWORK="mainnet"
  echo "Generating pusher signer into $KEY_DIR/pusher.key …" >&2
  KEYGEN_OUT="$(
    cd "$REPO_ROOT"
    KEYGEN_NETWORK="$KEYGEN_NETWORK" KEYGEN_OUT="$KEY_PATH" \
      cargo run -q -p autara-client --example keygen
  )" || die "keygen failed"
  echo "$KEYGEN_OUT" >&2
  PUBKEY="$(printf '%s\n' "$KEYGEN_OUT" | sed -n 's/^arch pubkey:[[:space:]]*//p')"
fi

# Encode the hex key file for Railway's SIGNER_KEY_B64 (entrypoint base64 -d).
base64 < "$KEY_PATH" | tr -d '\n' > "$B64_PATH"
chmod 600 "$B64_PATH"

if [ -z "$PUBKEY" ]; then
  PUBKEY="$(
    cd "$REPO_ROOT"
    cargo run -q -p autara-client --example print_pubkey -- --key "$KEY_PATH"
  )" || die "failed to derive pubkey from $KEY_PATH"
fi

if [ "$FUND" -eq 1 ]; then
  [ "$NETWORK" = "mainnet" ] && die "--fund refused on mainnet (no faucet)"
  RPC="https://rpc.testnet.arch.network"
  echo "Funding pusher signer from faucet ($RPC) …" >&2
  (
    cd "$REPO_ROOT"
    cargo run -q -p autara-client --example fund_signer -- \
      --key "$KEY_PATH" --rpc "$RPC" --network testnet
  ) || die "faucet funding failed"
fi

if [ "$COPY" -eq 1 ]; then
  command -v pbcopy >/dev/null 2>&1 || die "pbcopy not found (omit --copy and use $B64_PATH)"
  pbcopy < "$B64_PATH"
  echo "SIGNER_KEY_B64 copied to clipboard (not printed)." >&2
fi

if [ "$PRINT_B64" -eq 1 ]; then
  cat "$B64_PATH"
  echo
fi

cat <<EOF

Pusher signer ready
-------------------
network:          $NETWORK
secret key file:  $KEY_DIR/pusher.key   (mode 0600; NEVER commit)
SIGNER_KEY_B64:   $KEY_DIR/pusher.signer.b64  (mode 0600; paste into Railway)
arch pubkey:      $PUBKEY

Railway (testnet pusher service) — set these variables:
  ROLE=pusher
  NETWORK=testnet
  ARCH_RPC_URL=https://rpc.testnet.arch.network
  ORACLE_PROGRAM_ID=eee682c27db375bebbc17ed9a76aaa935c8b72bc7de50d736f03e2dfbed84b15
  FEEDS=0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43,0xeaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a
  SIGNER_KEY_B64=<contents of $KEY_DIR/pusher.signer.b64>
  PUSHER_PUBKEY=$PUBKEY   # optional, on the server for balance metrics

Next: upgrade the testnet oracle ELF, then let this pusher run one cycle.
See docs/oracle-pusher.md ("Testnet repair").
EOF
