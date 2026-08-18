#!/bin/sh
set -e

# One image, two roles, both networks. Everything is env-driven so the same
# container deploys to Arch mainnet and Arch testnet, differing only by env:
#
#   ROLE=server  (default) — the Autara API/indexer. Keeps its built-in pusher
#                            unless DISABLE_PRICE_PUSHER=1.
#   ROLE=pusher            — the dedicated oracle price pusher (autara-pyth),
#                            run once per network.
#
# Shared env:
#   NETWORK            testnet | mainnet            (default: testnet)
#   ARCH_RPC_URL       Arch JSON-RPC url            (per network; ARCH_NODE also accepted)
#   ORACLE_PROGRAM_ID  oracle program id (hex)      (per network)
#   SIGNER_KEY_B64     base64 of a hex secret-key file for the signer.
#                        pusher: REQUIRED on mainnet (no faucet); optional on
#                        testnet (a throwaway faucet-funded key is used if unset).
# Pusher-only:
#   FEEDS              comma-separated 0x… Pyth feed ids (default: BTC,USDC)
# Server-only:
#   PROGRAM_ID         lending program id (hex; falls back to compiled stage default)
#   DISABLE_PRICE_PUSHER=1  hand pushing to a dedicated ROLE=pusher service

# Decode secrets from environment variables (Railway/CI inject these; never on disk).
if [ -n "$TOKENS_JSON_B64" ]; then
  echo "$TOKENS_JSON_B64" | base64 -d > /app/tokens.json
fi

if [ -n "$SIGNER_KEY_B64" ]; then
  mkdir -p /app/keys
  echo "$SIGNER_KEY_B64" | base64 -d > /app/keys/signer.key
  export AUTARA_SIGNER_KEY=/app/keys/signer.key
fi

if [ -n "$PROGRAM_KEY_B64" ]; then
  mkdir -p /app/keys
  echo "$PROGRAM_KEY_B64" | base64 -d > /app/keys/autara-stage.key
fi

if [ -n "$ORACLE_KEY_B64" ]; then
  mkdir -p /app/keys
  echo "$ORACLE_KEY_B64" | base64 -d > /app/keys/autara-pyth-stage.key
fi

if [ -n "$TOKEN_AUTHORITY_KEY_B64" ]; then
  mkdir -p /app/keys
  echo "$TOKEN_AUTHORITY_KEY_B64" | base64 -d > /app/keys/autara-token-authority.key
fi

NETWORK="${NETWORK:-testnet}"
ROLE="${ROLE:-server}"
# ARCH_RPC_URL is the canonical name; keep ARCH_NODE working for existing deploys.
ARCH_RPC_URL="${ARCH_RPC_URL:-$ARCH_NODE}"

case "$ROLE" in
  pusher)
    : "${ARCH_RPC_URL:?ARCH_RPC_URL (or ARCH_NODE) required for ROLE=pusher}"
    : "${ORACLE_PROGRAM_ID:?ORACLE_PROGRAM_ID required for ROLE=pusher}"
    # autara-pyth parses the raw bitcoin::Network, which spells mainnet "bitcoin".
    PUSH_NETWORK="$NETWORK"
    [ "$PUSH_NETWORK" = "mainnet" ] && PUSH_NETWORK="bitcoin"
    FEEDS="${FEEDS:-0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43,0xeaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a}"
    SIGNER_ARG=""
    [ -f /app/keys/signer.key ] && SIGNER_ARG="--signer /app/keys/signer.key"
    # Stable signer is required in the container: throwaway faucet keys strand
    # feed authority after the first bind and make Railway restarts unsafe.
    if [ -z "$SIGNER_ARG" ]; then
      echo "ROLE=pusher requires SIGNER_KEY_B64 (stable signer; do not use throwaway faucet keys)" >&2
      exit 1
    fi
    # Railway injects PORT — bind /health + /metrics there for healthchecks.
    METRICS_LISTEN="0.0.0.0:${PORT:-9090}"
    echo "Starting oracle pusher: network=$PUSH_NETWORK oracle=$ORACLE_PROGRAM_ID metrics=$METRICS_LISTEN"
    exec autara-pyth \
      --network "$PUSH_NETWORK" \
      --rpc "$ARCH_RPC_URL" \
      --program-id "$ORACLE_PROGRAM_ID" \
      --feeds "$FEEDS" \
      --metrics-listen "$METRICS_LISTEN" \
      $SIGNER_ARG
    ;;

  server)
    # Railway injects PORT; default to 62776.
    LISTEN_ADDR="0.0.0.0:${PORT:-62776}"
    exec autara-server \
      --tokens "${TOKENS_PATH:-/app/tokens.json}" \
      --listen "$LISTEN_ADDR" \
      --network "$NETWORK" \
      ${PROGRAM_ID:+--program-id "$PROGRAM_ID"} \
      ${ORACLE_PROGRAM_ID:+--oracle-program-id "$ORACLE_PROGRAM_ID"} \
      ${ARCH_RPC_URL:+--arch-node "$ARCH_RPC_URL"}
    ;;

  *)
    echo "unknown ROLE=$ROLE (want 'server' or 'pusher')" >&2
    exit 1
    ;;
esac
